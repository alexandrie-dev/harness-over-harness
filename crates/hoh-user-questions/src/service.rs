use crate::{
    projection::{UserQuestionProjectionState, user_question_projection_definition},
    timed_wait::{TimedQuestionClaim, TimedQuestionWait},
    types::{
        AskUserQuestionAnswer, AskUserQuestionIntent, AskUserQuestionItem,
        AskUserQuestionRequestEvent, TimedUserQuestionResult, UserQuestionState, UserQuestionWait,
    },
};
use hoh_agent::{Agent, AgentInboxClaimed, AgentInboxDiscarded, Agents, agent_carrier};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service, Undo, Waterfall};
use hoh_llm::{
    ContentBlock, MessageId, MessageSource, NewUserMessage, ToolCallId, UserMessage,
    create_user_message,
};
use hoh_session::{Session, SessionEventEmitted};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use hoh_timeout::{AbortSignal, MAX_TIMER_DELAY_MS};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};
use tokio::time::Instant;

pub type AskUserQuestionRequest = AskUserQuestionRequestEvent;
hoh_kernel::event!(
    AskUserQuestionRequestEvent = "user-questions/request",
    Waterfall<AskUserQuestionAnswer>
);

pub struct UserQuestionError;
impl UserQuestionError {
    pub const NAME: &'static str = "UserQuestionError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}

fn aborted_question(cause: Option<BoxError>) -> HarnessError {
    let error = UserQuestionError::new(
        "ask_user_question was aborted before the user answered",
        "ASK_ABORTED",
    );
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

#[derive(Clone)]
struct QueuedReply {
    message_id: MessageId,
    claimed_turn: Option<u64>,
}

#[derive(Default)]
struct ServiceState {
    waits: HashMap<usize, HashMap<ToolCallId, Arc<TimedQuestionWait>>>,
    queued_replies: Vec<(Session, HashMap<ToolCallId, QueuedReply>)>,
}

struct Inner {
    ctx: Context,
    projection: Mutex<Option<ProjectionHandle<UserQuestionProjectionState>>>,
    state: Mutex<ServiceState>,
}

#[derive(Inject)]
struct ProjectionDeps {
    projections: Service<SessionProjections>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn agent_key(agent: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(agent) as *const () as usize
}

fn reply_call_id(message: &UserMessage) -> Option<ToolCallId> {
    (message.source.kind == "user-question-reply")
        .then(|| {
            message
                .source
                .data
                .get("callId")?
                .as_str()
                .map(ToolCallId::from)
        })
        .flatten()
}

impl Inner {
    fn close_waits(&self) {
        let waits = std::mem::take(&mut lock(&self.state).waits);
        for wait in waits.into_values().flat_map(HashMap::into_values) {
            wait.close(aborted_question(None));
        }
    }

    fn release_reply(&self, session: &Session, call_id: &ToolCallId, message_id: &MessageId) {
        let mut state = lock(&self.state);
        let Some(position) = state
            .queued_replies
            .iter()
            .position(|(candidate, _)| candidate == session)
        else {
            return;
        };
        let calls = &mut state.queued_replies[position].1;
        if calls
            .get(call_id)
            .is_some_and(|reply| &reply.message_id == message_id)
        {
            calls.remove(call_id);
        }
        if calls.is_empty() {
            state.queued_replies.remove(position);
        }
    }

    fn on_claimed(&self, event: &AgentInboxClaimed) {
        let Some(call_id) = reply_call_id(&event.message) else {
            return;
        };
        let mut state = lock(&self.state);
        let position = state
            .queued_replies
            .iter()
            .position(|(session, _)| session == event.agent.session())
            .unwrap_or_else(|| {
                state
                    .queued_replies
                    .push((event.agent.session().clone(), HashMap::new()));
                state.queued_replies.len() - 1
            });
        let calls = &mut state.queued_replies[position].1;
        match calls.get_mut(&call_id) {
            None => {
                calls.insert(
                    call_id,
                    QueuedReply {
                        message_id: event.message.id.clone(),
                        claimed_turn: Some(event.turn),
                    },
                );
            }
            Some(reply) if reply.message_id == event.message.id => {
                reply.claimed_turn = Some(event.turn);
            }
            Some(_) => {}
        }
    }

    fn on_session_event(&self, event: &SessionEventEmitted) {
        match event.event.event_type() {
            "user/message" => {
                let data = event.event.data();
                if data.pointer("/source/kind").and_then(Value::as_str)
                    != Some("user-question-reply")
                {
                    return;
                }
                let (Some(call_id), Some(message_id)) = (
                    data.pointer("/source/callId").and_then(Value::as_str),
                    data.get("id").and_then(Value::as_str),
                ) else {
                    return;
                };
                self.release_reply(
                    &event.session,
                    &ToolCallId::from(call_id),
                    &MessageId::from(message_id),
                );
            }
            "turn/end" => {
                let Some(turn) = event.event.data().get("turn").and_then(Value::as_u64) else {
                    return;
                };
                let mut state = lock(&self.state);
                let Some(position) = state
                    .queued_replies
                    .iter()
                    .position(|(session, _)| session == &event.session)
                else {
                    return;
                };
                let calls = &mut state.queued_replies[position].1;
                calls.retain(|_, reply| reply.claimed_turn != Some(turn));
                if calls.is_empty() {
                    state.queued_replies.remove(position);
                }
            }
            _ => {}
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.close_waits();
    }
}

struct WaitRegistrationGuard {
    inner: Weak<Inner>,
    key: usize,
    call_id: ToolCallId,
    wait: Arc<TimedQuestionWait>,
}

impl Drop for WaitRegistrationGuard {
    fn drop(&mut self) {
        self.wait.close(aborted_question(None));
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut state = lock(&inner.state);
        if let Some(calls) = state.waits.get_mut(&self.key) {
            if calls
                .get(&self.call_id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.wait))
            {
                calls.remove(&self.call_id);
            }
            if calls.is_empty() {
                state.waits.remove(&self.key);
            }
        }
    }
}

#[derive(Clone)]
pub struct UserQuestionService {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub UserQuestions: UserQuestionService = "userQuestions");

impl UserQuestionService {
    async fn start(ctx: Context) -> Result<Self, BoxError> {
        let projections_available = ctx.get::<SessionProjections>().is_some();
        let inner = Arc::new(Inner {
            ctx: ctx.clone(),
            projection: Mutex::new(None),
            state: Mutex::new(ServiceState::default()),
        });
        let weak = Arc::downgrade(&inner);
        ctx.effect("userQuestions.foregroundWaits", move || {
            Undo::sync(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.close_waits();
                }
            })
        })?;
        let weak = Arc::downgrade(&inner);
        let binding = ctx.inject(move |projection_ctx, deps: ProjectionDeps| {
            let weak = weak.clone();
            async move {
                let handle = deps
                    .projections
                    .register(&projection_ctx, user_question_projection_definition())
                    .await?;
                if let Some(inner) = weak.upgrade() {
                    *lock(&inner.projection) = Some(handle);
                }
                projection_ctx.effect("userQuestions.projection", move || {
                    Undo::sync(move || {
                        if let Some(inner) = weak.upgrade() {
                            *lock(&inner.projection) = None;
                        }
                    })
                })?;
                Ok(())
            }
        })?;
        if projections_available {
            binding.settled().await?;
        }
        let weak = Arc::downgrade(&inner);
        ctx.listen()
            .global()
            .on_contained::<AgentInboxClaimed>(move |event| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_claimed(event);
                }
                Ok(())
            })?;
        let weak = Arc::downgrade(&inner);
        ctx.listen()
            .global()
            .on_contained::<AgentInboxDiscarded>(move |event| {
                if let (Some(inner), Some(call_id)) =
                    (weak.upgrade(), reply_call_id(&event.message))
                {
                    inner.release_reply(event.agent.session(), &call_id, &event.message.id);
                }
                Ok(())
            })?;
        let weak = Arc::downgrade(&inner);
        ctx.listen()
            .global()
            .on_contained::<SessionEventEmitted>(move |event| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_session_event(event);
                }
                Ok(())
            })?;
        Ok(Self { inner })
    }

    fn assert_live_root(&self, agent: &Arc<dyn Agent>) -> Result<(), BoxError> {
        let Some(agents) = self.inner.ctx.get::<Agents>() else {
            return Err(UserQuestionError::new(
                "human interaction requires the exact live calling agent when an agent is supplied",
                "CALLER_NOT_LIVE",
            )
            .into());
        };
        let live = agents.get(agent.id());
        if !live.as_ref().is_some_and(|live| Arc::ptr_eq(live, agent)) {
            return Err(UserQuestionError::new(
                "human interaction requires the exact live calling agent when an agent is supplied",
                "CALLER_NOT_LIVE",
            )
            .into());
        }
        if agent
            .session()
            .header()
            .get("origin")
            .and_then(Value::as_str)
            == Some("subagent")
        {
            return Err(UserQuestionError::new(
                "human interaction is unavailable while the calling agent is owned by another live agent; include the unresolved question or decision in the child agent's final result",
                "DELEGATED_CALLER",
            )
            .into());
        }
        Ok(())
    }

    fn continued(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> Result<Vec<crate::PendingUserQuestion>, BoxError> {
        let projection = lock(&self.inner.projection).clone();
        let Some(projection) = projection else {
            return Ok(vec![]);
        };
        Ok(projection
            .state_of(agent.session())?
            .questions
            .active
            .iter()
            .filter(|question| question.state == UserQuestionState::Continued)
            .cloned()
            .collect())
    }

    pub fn answer(
        &self,
        agent: Arc<dyn Agent>,
        call_id: ToolCallId,
        answer: AskUserQuestionAnswer,
    ) -> Result<bool, BoxError> {
        self.assert_live_root(&agent)?;
        let Some(question) = self
            .continued(&agent)?
            .into_iter()
            .find(|question| question.call_id == call_id)
        else {
            return Ok(false);
        };
        let queued_in = |messages: Vec<UserMessage>| {
            messages
                .iter()
                .any(|message| reply_call_id(message).as_ref() == Some(&call_id))
        };
        let reserved = {
            let state = lock(&self.inner.state);
            state
                .queued_replies
                .iter()
                .find(|(session, _)| session == agent.session())
                .is_some_and(|(_, calls)| calls.contains_key(&call_id))
        };
        if reserved || queued_in(agent.inbox().next_turn()) || queued_in(agent.inbox().next_step())
        {
            return Err(UserQuestionError::new(
                "a reply is already queued for this question",
                "REPLY_QUEUED",
            )
            .into());
        }
        let answered = answer
            .answers
            .iter()
            .map(|item| item.id.as_str())
            .collect::<HashSet<_>>();
        if answered.len() != answer.answers.len()
            || question.questions.len() != answer.answers.len()
            || question
                .questions
                .iter()
                .any(|item| !answered.contains(item.id.as_str()))
        {
            return Err(UserQuestionError::new(
                format!(
                    "the answer batch for {call_id} must name each of its {} questions exactly once",
                    question.questions.len()
                ),
                "BAD_ANSWER",
            )
            .into());
        }
        let mut source = MessageSource::new("user-question-reply");
        source
            .data
            .insert("callId".into(), Value::String(call_id.to_string()));
        source
            .data
            .insert("outcome".into(), Value::String("answered".into()));
        let payload = json!({
            "kind":"answer_to_pending_question",
            "tool":"ask_user_question",
            "callId":call_id,
            "questions":question.questions,
            "answers":answer.answers,
        });
        let message = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(serde_json::to_string(&payload)?)],
            source,
        });
        let mut state = lock(&self.inner.state);
        let position = state
            .queued_replies
            .iter()
            .position(|(session, _)| session == agent.session())
            .unwrap_or_else(|| {
                state
                    .queued_replies
                    .push((agent.session().clone(), HashMap::new()));
                state.queued_replies.len() - 1
            });
        if state.queued_replies[position].1.contains_key(&call_id)
            || queued_in(agent.inbox().next_turn())
            || queued_in(agent.inbox().next_step())
        {
            if state.queued_replies[position].1.is_empty() {
                state.queued_replies.remove(position);
            }
            return Err(UserQuestionError::new(
                "a reply is already queued for this question",
                "REPLY_QUEUED",
            )
            .into());
        }
        state.queued_replies[position].1.insert(
            call_id.clone(),
            QueuedReply {
                message_id: message.id.clone(),
                claimed_turn: None,
            },
        );
        drop(state);
        if let Err(error) = agent.steer(message.clone()) {
            self.inner
                .release_reply(agent.session(), &call_id, &message.id);
            return Err(error);
        }
        Ok(true)
    }

    pub fn attach_wait(
        &self,
        agent: Arc<dyn Agent>,
        call_id: &ToolCallId,
        signal: AbortSignal,
    ) -> Result<Option<TimedQuestionClaim>, BoxError> {
        self.assert_live_root(&agent)?;
        let wait = lock(&self.inner.state)
            .waits
            .get(&agent_key(&agent))
            .and_then(|calls| calls.get(call_id))
            .cloned();
        Ok(wait.and_then(|wait| wait.attach(signal)))
    }

    pub async fn ask_timed(
        &self,
        mut request: AskUserQuestionRequest,
        call_id: ToolCallId,
        timeout_ms: u64,
    ) -> Result<TimedUserQuestionResult, BoxError> {
        if timeout_ms == 0 || timeout_ms > MAX_TIMER_DELAY_MS {
            return Err(UserQuestionError::new(
                "timeout must fit a positive platform timer",
                "BAD_TIMEOUT",
            )
            .into());
        }
        let agent = request.agent.clone().ok_or_else(|| -> BoxError {
            UserQuestionError::new(
                "human interaction requires the exact live calling agent when an agent is supplied",
                "CALLER_NOT_LIVE",
            )
            .into()
        })?;
        self.assert_live_root(&agent)?;
        let key = agent_key(&agent);
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(timeout_ms),
            request.signal.clone(),
            UserQuestionError::new(
                "ask_user_question timed out before the user answered",
                "ASK_TIMED_OUT",
            ),
        );
        {
            let mut state = lock(&self.inner.state);
            let calls = state.waits.entry(key).or_default();
            if calls.contains_key(&call_id) {
                return Err(UserQuestionError::new(
                    "the question call already has a foreground wait",
                    "DUPLICATE_WAIT",
                )
                .into());
            }
            calls.insert(call_id.clone(), wait.clone());
        }
        let _registration = WaitRegistrationGuard {
            inner: Arc::downgrade(&self.inner),
            key,
            call_id: call_id.clone(),
            wait: wait.clone(),
        };
        request.signal = Some(wait.signal());
        request.wait = Some(UserQuestionWait {
            call_id: call_id.clone(),
            timed: Some(true),
        });
        let mut result = self.ask(request).await;
        let no_provider = result
            .as_ref()
            .err()
            .and_then(|error| HarnessError::of(error.as_ref()))
            .is_some_and(|error| {
                error.is_named(UserQuestionError::NAME) && error.code() == "NO_PROVIDER"
            });
        if result.is_err() && wait.signal().aborted() {
            result = Err(Box::new(
                wait.signal()
                    .reason()
                    .expect("aborted wait has an abort reason"),
            ));
        } else if no_provider {
            wait.done().await;
            result = Err(Box::new(
                wait.signal()
                    .reason()
                    .expect("completed wait has an abort reason"),
            ));
        }
        match result {
            Ok(answer) => Ok(TimedUserQuestionResult::Answer(answer)),
            Err(error)
                if HarnessError::of(error.as_ref()).is_some_and(|error| {
                    error.is_named(UserQuestionError::NAME) && error.code() == "ASK_TIMED_OUT"
                }) =>
            {
                Ok(TimedUserQuestionResult::pending(call_id.clone()))
            }
            Err(error) if wait.signal().aborted() => Err(aborted_question(Some(error)).into()),
            Err(error) => Err(error),
        }
    }

    pub async fn ask(
        &self,
        request: AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, BoxError> {
        if request.signal.as_ref().is_some_and(AbortSignal::aborted) {
            return Err(aborted_question(None).into());
        }
        validate_nonempty(&request)?;
        if let Some(agent) = &request.agent {
            self.assert_live_root(agent)?;
        }
        validate_intents(&request)?;
        let agent = request.agent.clone();
        let dispatch = agent.as_ref().map_or_else(
            || self.inner.ctx.clone(),
            |agent| agent_carrier(agent).context(&self.inner.ctx),
        );
        let signal = request.signal.clone();
        let result = dispatch
            .waterfall(request, |_| async {
                Err(UserQuestionError::new(
                    "no user-questions answerer accepted the request",
                    "NO_PROVIDER",
                )
                .into())
            })
            .await;
        match result {
            Ok(answer) => Ok(answer),
            Err(error) => {
                if HarnessError::of(error.as_ref())
                    .is_some_and(|error| error.is_named(UserQuestionError::NAME))
                {
                    return Err(error);
                }
                if signal.as_ref().is_some_and(AbortSignal::aborted) {
                    return Err(aborted_question(Some(error)).into());
                }
                if HarnessError::of(error.as_ref()).is_some() {
                    return Err(error);
                }
                Err(UserQuestionError::new(error.to_string(), "PROVIDER_ERROR")
                    .with_cause(error)
                    .into())
            }
        }
    }
}

#[cfg(test)]
fn validate_request(request: &AskUserQuestionRequest) -> Result<(), BoxError> {
    validate_nonempty(request)?;
    validate_intents(request)
}

fn validate_nonempty(request: &AskUserQuestionRequest) -> Result<(), BoxError> {
    if request.questions.is_empty() {
        return Err(UserQuestionError::new(
            "ask_user_question requires at least one question",
            "EMPTY_QUESTIONS",
        )
        .into());
    }
    Ok(())
}

fn validate_intents(request: &AskUserQuestionRequest) -> Result<(), BoxError> {
    for question in &request.questions {
        validate_intent(question, request.questions.len(), request.wait.is_some())?;
    }
    Ok(())
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("string is JSON")
}

fn validate_intent(
    question: &AskUserQuestionItem,
    question_count: usize,
    has_wait: bool,
) -> Result<(), BoxError> {
    let Some(intent) = &question.intent else {
        return Ok(());
    };
    let options = question.options.as_deref().unwrap_or_default();
    let labels: Vec<&str> = match intent {
        AskUserQuestionIntent::PlanReview { approve, .. } => vec![approve],
        AskUserQuestionIntent::Approval {
            approve,
            reject,
            always,
            ..
        } => [Some(approve), Some(reject), always.as_ref()]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect(),
    };
    if let Some(unoffered) = labels
        .iter()
        .find(|label| !options.iter().any(|option| option.label == **label))
    {
        let message = match intent {
            AskUserQuestionIntent::PlanReview { approve, .. } => format!(
                "question {} declares intent plan-review whose approve label {} names none of its options",
                question.id,
                quoted(approve)
            ),
            AskUserQuestionIntent::Approval { .. } => format!(
                "question {} declares intent approval whose action label {} names none of its options",
                question.id,
                quoted(unoffered)
            ),
        };
        return Err(UserQuestionError::new(message, "BAD_INTENT").into());
    }
    if question.detail.is_none() {
        let detail = match intent {
            AskUserQuestionIntent::Approval { .. } => "scope detail it requires",
            AskUserQuestionIntent::PlanReview { .. } => "detail it reviews",
        };
        return Err(UserQuestionError::new(
            format!(
                "question {} declares intent {} without the {detail}",
                question.id,
                intent.kind()
            ),
            "BAD_INTENT",
        )
        .into());
    }
    if matches!(intent, AskUserQuestionIntent::Approval { .. }) {
        if question_count != 1 {
            return Err(UserQuestionError::new(
                format!("approval presentation requires one question; received {question_count}"),
                "BAD_INTENT",
            )
            .into());
        }
        if question.multi_select == Some(true) {
            return Err(UserQuestionError::new(
                format!(
                    "approval question {} does not support multiSelect",
                    question.id
                ),
                "BAD_INTENT",
            )
            .into());
        }
        if has_wait {
            return Err(UserQuestionError::new(
                format!(
                    "approval question {} requires a blocking request without wait",
                    question.id
                ),
                "BAD_INTENT",
            )
            .into());
        }
        if labels.iter().collect::<HashSet<_>>().len() != labels.len() {
            return Err(UserQuestionError::new(
                format!(
                    "approval question {} requires distinct action labels; received {}",
                    question.id,
                    serde_json::to_string(&labels)?
                ),
                "BAD_INTENT",
            )
            .into());
        }
        if options.len() != labels.len() {
            return Err(UserQuestionError::new(
                format!(
                    "approval question {} requires {} options matching its actions; received {}",
                    question.id,
                    labels.len(),
                    options.len()
                ),
                "BAD_INTENT",
            )
            .into());
        }
    }
    Ok(())
}

pub struct UserQuestionsPlugin;

impl Plugin for UserQuestionsPlugin {
    const NAME: &'static str = "UserQuestionService";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let service = UserQuestionService::start(ctx.clone()).await?;
        ctx.provide::<UserQuestions>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AskUserQuestionOption;
    use hoh_agent::{
        AgentOptions, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox, InboxTarget,
        MaintenanceFuture, MaintenanceTask, Model,
    };
    use hoh_kernel::Kernel;
    use hoh_scope::ScopeKey;
    use hoh_session::{
        AgentCancelCause, CreateSessionOptions, SessionPlugin, Sessions, SurfaceIntent,
    };
    use hoh_session_projection::SessionProjectionRegistry;
    use hoh_timeout::AbortController;
    use std::{
        error::Error,
        fmt,
        future::Future,
        pin::Pin,
        sync::{
            Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    #[derive(Debug)]
    struct EmptyError;
    impl fmt::Display for EmptyError {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Ok(())
        }
    }
    impl Error for EmptyError {}

    #[derive(Default)]
    struct TestInbox {
        next_turn: Mutex<Vec<UserMessage>>,
        next_step: Mutex<Vec<UserMessage>>,
        reservation_gate: Mutex<Option<Arc<AtomicUsize>>>,
    }
    impl Inbox for TestInbox {
        fn next_turn(&self) -> Vec<UserMessage> {
            if let Some(gate) = lock(&self.reservation_gate).clone() {
                gate.fetch_add(1, Ordering::SeqCst);
                let deadline = std::time::Instant::now() + Duration::from_millis(50);
                while gate.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
                    std::thread::yield_now();
                }
            }
            lock(&self.next_turn).clone()
        }
        fn next_step(&self) -> Vec<UserMessage> {
            lock(&self.next_step).clone()
        }
        fn clear(&self) -> Result<(), BoxError> {
            lock(&self.next_turn).clear();
            lock(&self.next_step).clear();
            Ok(())
        }
        fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
            match target {
                InboxTarget::NextTurn => lock(&self.next_turn).push(message),
                InboxTarget::NextStep => lock(&self.next_step).push(message),
            }
            Ok(())
        }
        fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
            match target {
                InboxTarget::NextTurn => lock(&self.next_turn).insert(0, message),
                InboxTarget::NextStep => lock(&self.next_step).insert(0, message),
            }
            Ok(())
        }
        fn replace(
            &self,
            message_id: &MessageId,
            new_message: UserMessage,
        ) -> Result<bool, BoxError> {
            for messages in [&self.next_turn, &self.next_step] {
                let mut messages = lock(messages);
                if let Some(index) = messages
                    .iter()
                    .position(|message| &message.id == message_id)
                {
                    messages[index] = new_message;
                    return Ok(true);
                }
            }
            Ok(false)
        }
        fn remove(&self, message_id: &MessageId) -> Result<bool, BoxError> {
            for messages in [&self.next_turn, &self.next_step] {
                let mut messages = lock(messages);
                if let Some(index) = messages
                    .iter()
                    .position(|message| &message.id == message_id)
                {
                    messages.remove(index);
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
            let slot = match target {
                InboxTarget::NextTurn => &self.next_turn,
                InboxTarget::NextStep => &self.next_step,
            };
            let mut messages = lock(slot);
            let start = start.max(0.0).min(messages.len() as f64) as usize;
            let end = start
                .saturating_add(delete_count.max(0.0) as usize)
                .min(messages.len());
            Ok(messages.splice(start..end, inserted).collect())
        }
    }

    struct TestAgent {
        session: Session,
        ctx: Context,
        key: ScopeKey,
        options: AgentOptions,
        inbox: TestInbox,
        fail_steer: AtomicBool,
    }
    impl Agent for TestAgent {
        fn id(&self) -> &hoh_session::SessionId {
            self.session.id()
        }
        fn scope_key(&self) -> ScopeKey {
            self.key.clone()
        }
        fn options(&self) -> &AgentOptions {
            &self.options
        }
        fn session(&self) -> &Session {
            &self.session
        }
        fn model(&self) -> &dyn Model {
            panic!("model is not used by user-question tests")
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
        fn when_idle(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
            Box::pin(async {})
        }
        fn run_maintenance_erased(
            &self,
            _: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            Err("maintenance is not used by user-question tests".into())
        }
        fn send(&self, message: UserMessage, target: InboxTarget, _: bool) -> Result<(), BoxError> {
            self.inbox.append(target, message)
        }
        fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
            self.inbox.append(InboxTarget::NextTurn, message)
        }
        fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
            if self.fail_steer.load(Ordering::SeqCst) {
                return Err("steering failed".into());
            }
            self.inbox.append(InboxTarget::NextStep, message)
        }
        fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
            self.inbox.append(InboxTarget::NextStep, message)
        }
    }

    struct AgentHarness {
        kernel: Kernel,
        ctx: Context,
        service: UserQuestionService,
        agent: Arc<TestAgent>,
    }
    impl AgentHarness {
        async fn new(origin: Option<&str>) -> Self {
            let kernel = Kernel::start();
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
            let service = UserQuestionService::start(ctx.clone()).await.unwrap();
            let meta = origin.map(|origin| json!({"origin":origin}));
            let session = ctx
                .get::<Sessions>()
                .unwrap()
                .create(
                    &ctx,
                    Some(format!("agent-{}", origin.unwrap_or("root")).into()),
                    CreateSessionOptions {
                        meta,
                        ..Default::default()
                    },
                )
                .unwrap();
            let agent = Arc::new(TestAgent {
                session,
                ctx: ctx.clone(),
                key: ScopeKey::new(),
                options: AgentOptions::default(),
                inbox: TestInbox::default(),
                fail_steer: AtomicBool::new(false),
            });
            ctx.get::<Agents>()
                .unwrap()
                .enter(&ctx, agent.clone())
                .unwrap();
            Self {
                kernel,
                ctx,
                service,
                agent,
            }
        }

        fn open_question(&self, call_id: &str, ids: &[&str]) {
            self.agent
                .session
                .append(
                    "request/header",
                    json!({"header":{"config":{},"tools":[{"name":"ask_user_question","description":"Ask","parameters":{"properties":{"timeout":{}}}}]},"reason":"initial"}),
                    None,
                )
                .unwrap();
            let questions = ids
                .iter()
                .map(|id| json!({"id":id,"question":format!("{id}?")}))
                .collect::<Vec<_>>();
            self.agent
                .session
                .append(
                    "tool/call",
                    json!({"turn":1,"step":1,"callId":call_id,"name":"ask_user_question","arguments":serde_json::to_string(&json!({"questions":questions})).unwrap()}),
                    None,
                )
                .unwrap();
        }

        fn continue_question(&self, call_id: &str, ids: &[&str]) {
            self.open_question(call_id, ids);
            let message = hoh_llm::create_tool_result_message(hoh_llm::ToolResultMessageInput {
                call_id: call_id.into(),
                content: vec![ContentBlock::text(
                    json!({"pending":true,"callId":call_id}).to_string(),
                )],
                is_error: false,
            });
            self.agent
                .session
                .append(
                    "tool/result",
                    json!({"turn":1,"step":1,"message":message}),
                    Some(SurfaceIntent::append()),
                )
                .unwrap();
        }

        async fn close(self) {
            self.kernel.shutdown().await;
        }
    }

    fn option(label: &str) -> AskUserQuestionOption {
        AskUserQuestionOption {
            label: label.into(),
            description: None,
        }
    }
    fn request(question: AskUserQuestionItem) -> AskUserQuestionRequest {
        AskUserQuestionRequest {
            questions: vec![question],
            agent: None,
            signal: None,
            wait: None,
        }
    }
    fn question(intent: Option<AskUserQuestionIntent>) -> AskUserQuestionItem {
        AskUserQuestionItem {
            id: "decision".into(),
            question: "Proceed?".into(),
            detail: Some("scope".into()),
            header: None,
            options: Some(vec![option("Allow"), option("Reject")]),
            multi_select: None,
            intent,
        }
    }
    fn code(error: &BoxError) -> &str {
        HarnessError::of(error.as_ref()).unwrap().code()
    }

    async fn live_service() -> (Kernel, Context, UserQuestionService) {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let service = UserQuestionService::start(ctx.clone()).await.unwrap();
        (kernel, ctx, service)
    }

    fn generic_request() -> AskUserQuestionRequest {
        request(question(None))
    }

    fn timed_request(agent: Arc<TestAgent>, signal: Option<AbortSignal>) -> AskUserQuestionRequest {
        AskUserQuestionRequest {
            questions: generic_request().questions,
            agent: Some(agent),
            signal,
            wait: None,
        }
    }

    fn answer_for(id: &str) -> AskUserQuestionAnswer {
        AskUserQuestionAnswer {
            answers: vec![crate::AskUserQuestionAnswerItem {
                id: id.into(),
                selected: vec!["yes".into()],
                custom: None,
            }],
        }
    }

    fn reply_for(call_id: &str, text: &str) -> UserMessage {
        let mut source = MessageSource::new("user-question-reply");
        source
            .data
            .insert("callId".into(), Value::String(call_id.into()));
        source
            .data
            .insert("outcome".into(), Value::String("answered".into()));
        create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source,
        })
    }

    fn dispatch_claimed(h: &AgentHarness, message: UserMessage, turn: u64) {
        h.ctx
            .root()
            .dispatch(AgentInboxClaimed {
                agent: h.agent.clone(),
                message,
                turn,
            })
            .invoke(|failure| panic!("claim listener failed: {failure:?}"));
    }

    fn dispatch_discarded(h: &AgentHarness, message: UserMessage) {
        h.ctx
            .root()
            .dispatch(AgentInboxDiscarded {
                agent: h.agent.clone(),
                message,
            })
            .invoke(|failure| panic!("discard listener failed: {failure:?}"));
    }

    #[test]
    fn error_has_stable_name_and_open_code() {
        let error = UserQuestionError::new("message", "FUTURE_CODE");
        assert_eq!(error.name(), "UserQuestionError");
        assert_eq!(error.code(), "FUTURE_CODE");
    }

    #[test]
    fn rejects_empty_questions() {
        let request = AskUserQuestionRequest {
            questions: vec![],
            agent: None,
            signal: None,
            wait: None,
        };
        assert_eq!(
            code(&validate_request(&request).unwrap_err()),
            "EMPTY_QUESTIONS"
        );
    }

    #[test]
    fn generic_question_needs_no_options_or_detail() {
        let mut q = question(None);
        q.options = None;
        q.detail = None;
        assert!(validate_request(&request(q)).is_ok());
    }

    #[test]
    fn plan_review_requires_offered_approve_label() {
        let q = question(Some(AskUserQuestionIntent::PlanReview {
            approve: "Missing".into(),
            call_id: None,
        }));
        let error = validate_request(&request(q)).unwrap_err();
        assert_eq!(code(&error), "BAD_INTENT");
        assert!(error.to_string().contains("approve label \"Missing\""));
    }

    #[test]
    fn plan_review_requires_detail() {
        let mut q = question(Some(AskUserQuestionIntent::PlanReview {
            approve: "Allow".into(),
            call_id: None,
        }));
        q.detail = None;
        assert!(
            validate_request(&request(q))
                .unwrap_err()
                .to_string()
                .contains("detail it reviews")
        );
    }

    fn approval(always: Option<&str>) -> AskUserQuestionIntent {
        AskUserQuestionIntent::Approval {
            approve: "Allow".into(),
            reject: "Reject".into(),
            always: always.map(Into::into),
            subject: None,
            display: None,
        }
    }

    fn source_approval_question() -> AskUserQuestionItem {
        AskUserQuestionItem {
            id: "access".into(),
            question: "Allow access?".into(),
            detail: Some("Scope of access".into()),
            header: None,
            options: Some(vec![option("Task"), option("Deny"), option("Always")]),
            multi_select: None,
            intent: Some(AskUserQuestionIntent::Approval {
                approve: "Task".into(),
                reject: "Deny".into(),
                always: Some("Always".into()),
                subject: None,
                display: None,
            }),
        }
    }

    #[test]
    fn approval_diagnostics_match_the_source_exactly() {
        for (kind, expected) in [
            (
                "unoffered",
                "question access declares intent approval whose action label \"Unknown\" names none of its options",
            ),
            (
                "duplicate",
                "approval question access requires distinct action labels; received [\"Task\",\"Deny\",\"Task\"]",
            ),
            (
                "extra",
                "approval question access requires 3 options matching its actions; received 4",
            ),
            (
                "multiple",
                "approval presentation requires one question; received 2",
            ),
            (
                "multiselect",
                "approval question access does not support multiSelect",
            ),
            (
                "timed",
                "approval question access requires a blocking request without wait",
            ),
            (
                "detail",
                "question access declares intent approval without the scope detail it requires",
            ),
        ] {
            let mut item = source_approval_question();
            match kind {
                "unoffered" => {
                    let Some(AskUserQuestionIntent::Approval { approve, .. }) = &mut item.intent
                    else {
                        unreachable!()
                    };
                    *approve = "Unknown".into();
                }
                "duplicate" => {
                    let Some(AskUserQuestionIntent::Approval { always, .. }) = &mut item.intent
                    else {
                        unreachable!()
                    };
                    *always = Some("Task".into());
                }
                "extra" => item
                    .options
                    .as_mut()
                    .unwrap()
                    .push(option("Something else")),
                "multiselect" => item.multi_select = Some(true),
                "detail" => item.detail = None,
                _ => {}
            }
            let questions = if kind == "multiple" {
                let mut other = item.clone();
                other.id = "other".into();
                vec![item, other]
            } else {
                vec![item]
            };
            let request = AskUserQuestionRequest {
                questions,
                agent: None,
                signal: None,
                wait: (kind == "timed").then(|| UserQuestionWait {
                    call_id: "call".into(),
                    timed: None,
                }),
            };
            let error = validate_request(&request).unwrap_err();
            assert_eq!(code(&error), "BAD_INTENT");
            assert_eq!(error.to_string(), expected, "{kind}");
        }
    }

    #[test]
    fn approval_accepts_fixed_choices_with_or_without_persistent_action() {
        let persistent = source_approval_question();
        assert!(validate_request(&request(persistent.clone())).is_ok());
        let mut transient = persistent;
        transient.options.as_mut().unwrap().pop();
        let Some(AskUserQuestionIntent::Approval { always, .. }) = &mut transient.intent else {
            unreachable!()
        };
        *always = None;
        assert!(validate_request(&request(transient)).is_ok());
    }

    #[test]
    fn valid_two_action_approval_is_accepted() {
        assert!(validate_request(&request(question(Some(approval(None))))).is_ok());
    }

    #[test]
    fn approval_requires_every_action_label_to_be_offered() {
        let mut q = question(Some(approval(Some("Always"))));
        q.options = Some(vec![option("Allow"), option("Reject")]);
        let error = validate_request(&request(q)).unwrap_err();
        assert!(error.to_string().contains("action label \"Always\""));
    }

    #[test]
    fn approval_requires_exactly_one_question() {
        let q = question(Some(approval(None)));
        let request = AskUserQuestionRequest {
            questions: vec![q.clone(), q],
            agent: None,
            signal: None,
            wait: None,
        };
        assert_eq!(
            validate_request(&request).unwrap_err().to_string(),
            "approval presentation requires one question; received 2"
        );
    }

    #[test]
    fn approval_rejects_multi_select() {
        let mut q = question(Some(approval(None)));
        q.multi_select = Some(true);
        assert!(
            validate_request(&request(q))
                .unwrap_err()
                .to_string()
                .contains("multiSelect")
        );
    }

    #[test]
    fn approval_rejects_timed_or_indefinite_wait_descriptor() {
        let mut request = request(question(Some(approval(None))));
        request.wait = Some(UserQuestionWait {
            call_id: "c".into(),
            timed: Some(true),
        });
        assert!(
            validate_request(&request)
                .unwrap_err()
                .to_string()
                .contains("without wait")
        );
    }

    #[test]
    fn approval_requires_distinct_actions() {
        let mut q = question(Some(AskUserQuestionIntent::Approval {
            approve: "Allow".into(),
            reject: "Allow".into(),
            always: None,
            subject: None,
            display: None,
        }));
        q.options = Some(vec![option("Allow"), option("Allow")]);
        assert_eq!(
            validate_request(&request(q)).unwrap_err().to_string(),
            "approval question decision requires distinct action labels; received [\"Allow\",\"Allow\"]"
        );
    }

    #[test]
    fn approval_rejects_extra_options() {
        let mut q = question(Some(approval(None)));
        q.options.as_mut().unwrap().push(option("Other"));
        assert_eq!(
            validate_request(&request(q)).unwrap_err().to_string(),
            "approval question decision requires 2 options matching its actions; received 3"
        );
    }

    #[test]
    fn approval_requires_scope_detail() {
        let mut q = question(Some(approval(None)));
        q.detail = None;
        assert!(
            validate_request(&request(q))
                .unwrap_err()
                .to_string()
                .contains("scope detail it requires")
        );
    }

    #[tokio::test]
    async fn reports_no_provider_when_the_waterfall_is_unclaimed() {
        let (kernel, _, service) = live_service().await;
        let error = service.ask(generic_request()).await.unwrap_err();
        assert_eq!(code(&error), "NO_PROVIDER");
        assert_eq!(
            error.to_string(),
            "no user-questions answerer accepted the request"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn composes_answerers_as_a_waterfall_until_one_claims() {
        let (kernel, ctx, service) = live_service().await;
        let delegated = Arc::new(AtomicUsize::new(0));
        let count = delegated.clone();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            move |_, next| {
                count.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            },
        )
        .unwrap();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            |request, _| async move {
                Ok(AskUserQuestionAnswer {
                    answers: vec![crate::AskUserQuestionAnswerItem {
                        id: request.questions[0].id.clone(),
                        selected: vec!["typed".into()],
                        custom: None,
                    }],
                })
            },
        )
        .unwrap();
        let answer = service.ask(generic_request()).await.unwrap();
        assert_eq!(delegated.load(Ordering::SeqCst), 1);
        assert_eq!(answer.answers[0].selected, ["typed"]);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn preserves_user_question_errors_from_answerers() {
        let (kernel, ctx, service) = live_service().await;
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
            Err(UserQuestionError::new("dismissed", "ASK_CANCELLED").into())
        })
        .unwrap();
        let error = service.ask(generic_request()).await.unwrap_err();
        assert_eq!(code(&error), "ASK_CANCELLED");
        assert_eq!(error.to_string(), "dismissed");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn preserves_other_coded_harness_errors() {
        let (kernel, ctx, service) = live_service().await;
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
            Err(HarnessError::named("ProviderDomainError", "domain", "DOMAIN").into())
        })
        .unwrap();
        let error = service.ask(generic_request()).await.unwrap_err();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.name(), "ProviderDomainError");
        assert_eq!(harness.code(), "DOMAIN");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn wraps_generic_provider_failure_with_message_and_cause() {
        let (kernel, ctx, service) = live_service().await;
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
            Err::<AskUserQuestionAnswer, BoxError>("display broke".into())
        })
        .unwrap();
        let error = service.ask(generic_request()).await.unwrap_err();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.code(), "PROVIDER_ERROR");
        assert_eq!(harness.message(), "display broke");
        assert_eq!(harness.cause().unwrap().to_string(), "display broke");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn preserves_an_empty_generic_provider_message() {
        let (kernel, ctx, service) = live_service().await;
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
            Err::<AskUserQuestionAnswer, BoxError>(Box::new(EmptyError))
        })
        .unwrap();
        let error = service.ask(generic_request()).await.unwrap_err();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.code(), "PROVIDER_ERROR");
        assert_eq!(harness.message(), "");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn pre_aborted_request_never_reaches_an_answerer() {
        let (kernel, ctx, service) = live_service().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Err::<AskUserQuestionAnswer, BoxError>("unexpected".into()) }
        })
        .unwrap();
        let control = AbortController::new();
        control.abort_with("cancelled before ask");
        let mut request = generic_request();
        request.signal = Some(control.signal());
        let error = service.ask(request).await.unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn in_flight_abort_maps_a_generic_answerer_failure_to_ask_aborted() {
        let (kernel, ctx, service) = live_service().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let observed = entered.clone();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            move |request, _| {
                let observed = observed.clone();
                async move {
                    observed.notify_one();
                    let signal = request.signal.as_ref().unwrap();
                    let _ = signal.cancelled().await;
                    Err::<AskUserQuestionAnswer, BoxError>("provider stopped".into())
                }
            },
        )
        .unwrap();
        let control = AbortController::new();
        let mut request = generic_request();
        request.signal = Some(control.signal());
        let pending = tokio::spawn({
            let service = service.clone();
            async move { service.ask(request).await }
        });
        entered.notified().await;
        control.abort_with("turn cancelled");
        let error = pending.await.unwrap().unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        assert_eq!(
            HarnessError::of(error.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .to_string(),
            "provider stopped"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn domain_error_wins_when_the_provider_also_aborts_the_signal() {
        let (kernel, ctx, service) = live_service().await;
        let control = Arc::new(AbortController::new());
        let abort = control.clone();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
            abort.abort();
            async { Err(UserQuestionError::new("cancelled", "ASK_CANCELLED").into()) }
        })
        .unwrap();
        let mut request = generic_request();
        request.signal = Some(control.signal());
        let error = service.ask(request).await.unwrap_err();
        assert_eq!(code(&error), "ASK_CANCELLED");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn live_caller_errors_precede_invalid_plan_review_intent() {
        let owner = AgentHarness::new(None).await;
        let (kernel, _, service) = live_service().await;
        let mut invalid = request(question(Some(AskUserQuestionIntent::PlanReview {
            approve: "Missing".into(),
            call_id: None,
        })));
        invalid.agent = Some(owner.agent.clone());
        assert_eq!(
            code(&service.ask(invalid).await.unwrap_err()),
            "CALLER_NOT_LIVE"
        );
        kernel.shutdown().await;
        owner.close().await;

        let delegated = AgentHarness::new(Some("subagent")).await;
        let mut invalid = request(question(Some(AskUserQuestionIntent::PlanReview {
            approve: "Missing".into(),
            call_id: None,
        })));
        invalid.agent = Some(delegated.agent.clone());
        assert_eq!(
            code(&delegated.service.ask(invalid).await.unwrap_err()),
            "DELEGATED_CALLER"
        );
        delegated.close().await;
    }

    #[tokio::test]
    async fn plan_review_without_options_is_bad_intent() {
        let (kernel, _, service) = live_service().await;
        let mut question = question(Some(AskUserQuestionIntent::PlanReview {
            approve: "Allow".into(),
            call_id: None,
        }));
        question.options = None;
        let error = service.ask(request(question)).await.unwrap_err();
        assert_eq!(code(&error), "BAD_INTENT");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn valid_plan_review_intent_reaches_the_waterfall_unchanged() {
        let (kernel, ctx, service) = live_service().await;
        let seen = Arc::new(Mutex::new(None));
        let output = seen.clone();
        ctx.on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            move |request, _| {
                *lock(&output) = request.questions[0].intent.clone();
                async { Ok(AskUserQuestionAnswer { answers: vec![] }) }
            },
        )
        .unwrap();
        let intent = AskUserQuestionIntent::PlanReview {
            approve: "Allow".into(),
            call_id: Some("plan-call".into()),
        };
        service
            .ask(request(question(Some(intent.clone()))))
            .await
            .unwrap();
        assert_eq!(*lock(&seen), Some(intent));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_live_subagent_before_reaching_the_provider() {
        let h = AgentHarness::new(Some("subagent")).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                async { Ok(AskUserQuestionAnswer { answers: vec![] }) }
            })
            .unwrap();
        let mut request = generic_request();
        request.agent = Some(h.agent.clone());
        let error = h.service.ask(request).await.unwrap_err();
        assert_eq!(code(&error), "DELEGATED_CALLER");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        h.close().await;
    }

    #[tokio::test]
    async fn reaches_the_provider_for_an_ordinary_live_root() {
        let h = AgentHarness::new(None).await;
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
                |request, _| async move {
                    assert!(request.agent.is_some());
                    Ok(AskUserQuestionAnswer { answers: vec![] })
                },
            )
            .unwrap();
        let mut request = generic_request();
        request.agent = Some(h.agent.clone());
        assert!(h.service.ask(request).await.is_ok());
        h.close().await;
    }

    #[tokio::test]
    async fn rejects_a_supplied_agent_without_a_live_registry() {
        let owner = AgentHarness::new(None).await;
        let (kernel, _, service) = live_service().await;
        let mut request = generic_request();
        request.agent = Some(owner.agent.clone());
        assert_eq!(
            code(&service.ask(request).await.unwrap_err()),
            "CALLER_NOT_LIVE"
        );
        kernel.shutdown().await;
        owner.close().await;
    }

    #[tokio::test]
    async fn rejects_a_stale_agent_object_that_reuses_a_live_id() {
        let h = AgentHarness::new(None).await;
        let stale = Arc::new(TestAgent {
            session: h.agent.session.clone(),
            ctx: h.ctx.clone(),
            key: ScopeKey::new(),
            options: AgentOptions::default(),
            inbox: TestInbox::default(),
            fail_steer: AtomicBool::new(false),
        });
        let mut request = generic_request();
        request.agent = Some(stale);
        assert_eq!(
            code(&h.service.ask(request).await.unwrap_err()),
            "CALLER_NOT_LIVE"
        );
        h.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn holds_an_unclaimed_timed_request_until_deadline_then_returns_pending() {
        let h = AgentHarness::new(None).await;
        let request = AskUserQuestionRequest {
            questions: generic_request().questions,
            agent: Some(h.agent.clone()),
            signal: None,
            wait: None,
        };
        let task = tokio::spawn({
            let service = h.service.clone();
            async move { service.ask_timed(request, "timed".into(), 5_000).await }
        });
        tokio::time::advance(Duration::from_millis(4_999)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        let result = task.await.unwrap().unwrap();
        assert_eq!(result, TimedUserQuestionResult::pending("timed".into()));
        h.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn sibling_waits_are_independent_and_unload_aborts_the_remainder() {
        let h = AgentHarness::new(None).await;
        let first = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), None);
            async move { service.ask_timed(request, "first".into(), 1_000).await }
        });
        let later = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), None);
            async move { service.ask_timed(request, "later".into(), 2_000).await }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1_000)).await;
        assert_eq!(
            first.await.unwrap().unwrap(),
            TimedUserQuestionResult::pending("first".into())
        );
        let claim = h
            .service
            .attach_wait(
                h.agent.clone(),
                &ToolCallId::from("later"),
                AbortController::new().signal(),
            )
            .unwrap()
            .unwrap();
        h.kernel.shutdown().await;
        let error = later.await.unwrap().unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        claim.ended().await;
    }

    #[tokio::test]
    async fn cancelling_ask_timed_future_removes_its_registration() {
        let h = AgentHarness::new(None).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let observed = entered.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                observed.notify_one();
                std::future::pending::<Result<AskUserQuestionAnswer, BoxError>>()
            })
            .unwrap();
        let task = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), None);
            async move {
                service
                    .ask_timed(request, "cancelled".into(), 120_000)
                    .await
            }
        });
        entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let turn = AbortController::new();
        turn.abort();
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), Some(turn.signal())),
                "cancelled".into(),
                120_000,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        h.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn forwarded_timed_signal_expires_without_aborting_the_turn_signal() {
        let h = AgentHarness::new(None).await;
        let forwarded = Arc::new(Mutex::new(None::<AbortSignal>));
        let output = forwarded.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let observed = entered.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
                move |request, _| {
                    let signal = request.signal.as_ref().unwrap().clone();
                    *lock(&output) = Some(signal.clone());
                    observed.notify_one();
                    async move {
                        let _ = signal.cancelled().await;
                        Err::<AskUserQuestionAnswer, BoxError>("forwarded request aborted".into())
                    }
                },
            )
            .unwrap();
        let turn = AbortController::new();
        let task = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), Some(turn.signal()));
            async move { service.ask_timed(request, "forwarded".into(), 5_000).await }
        });
        entered.notified().await;
        tokio::time::advance(Duration::from_millis(5_000)).await;
        assert_eq!(
            task.await.unwrap().unwrap(),
            TimedUserQuestionResult::pending("forwarded".into())
        );
        assert!(lock(&forwarded).as_ref().unwrap().aborted());
        assert!(!turn.signal().aborted());
        h.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn client_claim_holds_wait_and_closes_when_answer_settles() {
        let h = AgentHarness::new(None).await;
        let (send, receive) = tokio::sync::oneshot::channel();
        let receive = Arc::new(Mutex::new(Some(receive)));
        let input = receive.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let observed = entered.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                let receive = lock(&input).take().unwrap();
                observed.notify_one();
                async move { receive.await.unwrap() }
            })
            .unwrap();
        let task = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), None);
            async move { service.ask_timed(request, "claimed".into(), 5_000).await }
        });
        entered.notified().await;
        let claim = h
            .service
            .attach_wait(
                h.agent.clone(),
                &ToolCallId::from("claimed"),
                AbortController::new().signal(),
            )
            .unwrap()
            .unwrap();
        tokio::time::advance(Duration::from_millis(10_000)).await;
        assert!(!task.is_finished());
        let duplicate = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), None),
                "claimed".into(),
                5_000,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&duplicate), "DUPLICATE_WAIT");
        send.send(Ok(AskUserQuestionAnswer { answers: vec![] }))
            .unwrap();
        assert!(matches!(
            task.await.unwrap().unwrap(),
            TimedUserQuestionResult::Answer(_)
        ));
        claim.ended().await;
        assert!(
            h.service
                .attach_wait(
                    h.agent.clone(),
                    &ToolCallId::from("claimed"),
                    AbortController::new().signal(),
                )
                .unwrap()
                .is_none()
        );
        h.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_timed_calls_atomically_reserve_one_wait() {
        let h = AgentHarness::new(None).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let turn = Arc::new(AbortController::new());
        let spawn = |barrier: Arc<tokio::sync::Barrier>| {
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), Some(turn.signal()));
            tokio::spawn(async move {
                barrier.wait().await;
                service
                    .ask_timed(request, "same-wait".into(), 120_000)
                    .await
            })
        };
        let first = spawn(barrier.clone());
        let second = spawn(barrier.clone());
        barrier.wait().await;
        let mut first = first;
        let mut second = second;
        let duplicate = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::select! {
                result = &mut first => (result, second),
                result = &mut second => (result, first),
            }
        })
        .await
        .unwrap();
        let error = duplicate.0.unwrap().unwrap_err();
        assert_eq!(code(&error), "DUPLICATE_WAIT");
        turn.abort();
        let error = duplicate.1.await.unwrap().unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        h.close().await;
    }

    #[tokio::test]
    async fn returns_the_answer_when_a_provider_settles_inside_the_window() {
        let h = AgentHarness::new(None).await;
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
                |request, _| async move {
                    assert_eq!(request.wait.as_ref().unwrap().timed, Some(true));
                    Ok(AskUserQuestionAnswer {
                        answers: vec![crate::AskUserQuestionAnswerItem {
                            id: "decision".into(),
                            selected: vec!["Allow".into()],
                            custom: None,
                        }],
                    })
                },
            )
            .unwrap();
        let result = h
            .service
            .ask_timed(
                AskUserQuestionRequest {
                    questions: generic_request().questions,
                    agent: Some(h.agent.clone()),
                    signal: None,
                    wait: None,
                },
                "timed".into(),
                120_000,
            )
            .await
            .unwrap();
        assert!(matches!(result, TimedUserQuestionResult::Answer(_)));
        h.close().await;
    }

    #[tokio::test]
    async fn timed_out_provider_code_returns_pending_and_other_code_is_rethrown() {
        let h = AgentHarness::new(None).await;
        let timed_out = Arc::new(AtomicBool::new(true));
        let selected = timed_out.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                let code = if selected.load(Ordering::SeqCst) {
                    "ASK_TIMED_OUT"
                } else {
                    "ASK_CANCELLED"
                };
                async move { Err(UserQuestionError::new("client rejected", code).into()) }
            })
            .unwrap();
        assert_eq!(
            h.service
                .ask_timed(
                    timed_request(h.agent.clone(), None),
                    "mapped".into(),
                    120_000,
                )
                .await
                .unwrap(),
            TimedUserQuestionResult::pending("mapped".into())
        );
        timed_out.store(false, Ordering::SeqCst);
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), None),
                "mapped".into(),
                120_000,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), "ASK_CANCELLED");
        h.close().await;
    }

    #[tokio::test]
    async fn foreign_no_provider_code_is_not_treated_as_the_unclaimed_sentinel() {
        let h = AgentHarness::new(None).await;
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
                Err(HarnessError::named("ForeignProvider", "foreign", "NO_PROVIDER").into())
            })
            .unwrap();
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), None),
                "foreign".into(),
                120_000,
            )
            .await
            .unwrap_err();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.name(), "ForeignProvider");
        assert_eq!(harness.code(), "NO_PROVIDER");
        h.close().await;
    }

    #[tokio::test]
    async fn foreign_ask_timed_out_code_is_not_mapped_to_pending() {
        let h = AgentHarness::new(None).await;
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
                Err(
                    HarnessError::named("ForeignProvider", "foreign timeout", "ASK_TIMED_OUT")
                        .into(),
                )
            })
            .unwrap();
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), None),
                "foreign-timeout".into(),
                120_000,
            )
            .await
            .unwrap_err();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.name(), "ForeignProvider");
        assert_eq!(harness.code(), "ASK_TIMED_OUT");
        h.close().await;
    }

    #[tokio::test]
    async fn turn_signal_aborts_an_unclaimed_timed_hold() {
        let h = AgentHarness::new(None).await;
        let turn = Arc::new(AbortController::new());
        let task = tokio::spawn({
            let service = h.service.clone();
            let request = timed_request(h.agent.clone(), Some(turn.signal()));
            async move {
                service
                    .ask_timed(request, "turn-abort".into(), 120_000)
                    .await
            }
        });
        tokio::task::yield_now().await;
        turn.abort_with("turn ended");
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        h.close().await;
    }

    #[tokio::test]
    async fn generic_timed_provider_failure_is_provider_error() {
        let h = AgentHarness::new(None).await;
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
                Err::<AskUserQuestionAnswer, BoxError>("provider crashed".into())
            })
            .unwrap();
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), None),
                "provider-error".into(),
                120_000,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), "PROVIDER_ERROR");
        assert_eq!(error.to_string(), "provider crashed");
        h.close().await;
    }

    #[tokio::test]
    async fn aborted_turn_does_not_enter_the_no_provider_hold() {
        let h = AgentHarness::new(None).await;
        let turn = Arc::new(AbortController::new());
        let abort = turn.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                abort.abort_with("turn ended");
                async { Err(UserQuestionError::new("unclaimed", "NO_PROVIDER").into()) }
            })
            .unwrap();
        let error = h
            .service
            .ask_timed(
                timed_request(h.agent.clone(), Some(turn.signal())),
                "pre-aborted".into(),
                120_000,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), "ASK_ABORTED");
        h.close().await;
    }

    #[tokio::test]
    async fn rejects_zero_timed_wait_before_dispatching() {
        let h = AgentHarness::new(None).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        h.ctx
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                async { Ok(AskUserQuestionAnswer { answers: vec![] }) }
            })
            .unwrap();
        let error = h
            .service
            .ask_timed(
                AskUserQuestionRequest {
                    questions: generic_request().questions,
                    agent: Some(h.agent.clone()),
                    signal: None,
                    wait: None,
                },
                "timed".into(),
                0,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), "BAD_TIMEOUT");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        h.close().await;
    }

    #[tokio::test]
    async fn steers_a_complete_answer_into_a_continued_question() {
        let h = AgentHarness::new(None).await;
        h.continue_question("continued", &["one"]);
        let accepted = h
            .service
            .answer(
                h.agent.clone(),
                "continued".into(),
                AskUserQuestionAnswer {
                    answers: vec![crate::AskUserQuestionAnswerItem {
                        id: "one".into(),
                        selected: vec!["yes".into()],
                        custom: None,
                    }],
                },
            )
            .unwrap();
        assert!(accepted);
        let message = lock(&h.agent.inbox.next_step)[0].clone();
        assert_eq!(message.source.kind, "user-question-reply");
        assert_eq!(message.source.data["callId"], "continued");
        let payload: Value =
            serde_json::from_str(message.content[0].text_value().unwrap()).unwrap();
        assert_eq!(payload["kind"], "answer_to_pending_question");
        h.agent
            .session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        assert!(
            !h.service
                .answer(
                    h.agent.clone(),
                    "continued".into(),
                    AskUserQuestionAnswer { answers: vec![] },
                )
                .unwrap()
        );
        h.close().await;
    }

    #[tokio::test]
    async fn refuses_a_second_reply_while_the_first_is_queued() {
        let h = AgentHarness::new(None).await;
        h.continue_question("continued", &["one"]);
        let answer = AskUserQuestionAnswer {
            answers: vec![crate::AskUserQuestionAnswerItem {
                id: "one".into(),
                selected: vec![],
                custom: None,
            }],
        };
        assert!(
            h.service
                .answer(h.agent.clone(), "continued".into(), answer.clone())
                .unwrap()
        );
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "continued".into(), answer)
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        h.close().await;
    }

    #[tokio::test]
    async fn queued_reply_precedes_bad_answer_validation() {
        let h = AgentHarness::new(None).await;
        h.continue_question("continued", &["one"]);
        assert!(
            h.service
                .answer(h.agent.clone(), "continued".into(), answer_for("one"))
                .unwrap()
        );
        let error = h
            .service
            .answer(
                h.agent.clone(),
                "continued".into(),
                AskUserQuestionAnswer { answers: vec![] },
            )
            .unwrap_err();
        assert_eq!(code(&error), "REPLY_QUEUED");
        h.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_answers_atomically_reserve_one_reply() {
        let h = AgentHarness::new(None).await;
        h.continue_question("concurrent", &["one"]);
        *lock(&h.agent.inbox.reservation_gate) = Some(Arc::new(AtomicUsize::new(0)));
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let barrier = barrier.clone();
            let service = h.service.clone();
            let agent = h.agent.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                service.answer(agent, "concurrent".into(), answer_for("one"))
            }));
        }
        barrier.wait();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(true)))
                .count(),
            1
        );
        let error = results.into_iter().find_map(Result::err).unwrap();
        assert_eq!(code(&error), "REPLY_QUEUED");
        assert_eq!(lock(&h.agent.inbox.next_step).len(), 1);
        h.close().await;
    }

    #[tokio::test]
    async fn different_continued_batches_can_queue_replies_together() {
        let h = AgentHarness::new(None).await;
        h.continue_question("first-batch", &["one"]);
        h.open_question("second-batch", &["two"]);
        let message = hoh_llm::create_tool_result_message(hoh_llm::ToolResultMessageInput {
            call_id: "second-batch".into(),
            content: vec![ContentBlock::text(
                json!({"pending":true,"callId":"second-batch"}).to_string(),
            )],
            is_error: false,
        });
        h.agent
            .session
            .append(
                "tool/result",
                json!({"turn":1,"step":1,"message":message}),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        assert!(
            h.service
                .answer(h.agent.clone(), "second-batch".into(), answer_for("two"))
                .unwrap()
        );
        assert!(
            h.service
                .answer(h.agent.clone(), "first-batch".into(), answer_for("one"))
                .unwrap()
        );
        let calls = lock(&h.agent.inbox.next_step)
            .iter()
            .map(|message| message.source.data["callId"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(calls, ["second-batch", "first-batch"]);
        h.close().await;
    }

    #[tokio::test]
    async fn discarded_reply_releases_reservation_for_retry() {
        let h = AgentHarness::new(None).await;
        h.continue_question("discarded", &["one"]);
        assert!(
            h.service
                .answer(h.agent.clone(), "discarded".into(), answer_for("one"))
                .unwrap()
        );
        let first = lock(&h.agent.inbox.next_step).remove(0);
        dispatch_discarded(&h, first);
        assert!(
            h.service
                .answer(h.agent.clone(), "discarded".into(), answer_for("one"))
                .unwrap()
        );
        h.close().await;
    }

    #[tokio::test]
    async fn durable_inbox_reply_blocks_both_next_step_and_next_turn() {
        let h = AgentHarness::new(None).await;
        h.continue_question("inbox", &["one"]);
        let reply = reply_for("inbox", "queued");
        lock(&h.agent.inbox.next_step).push(reply.clone());
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "inbox".into(), answer_for("one"))
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        lock(&h.agent.inbox.next_step).clear();
        lock(&h.agent.inbox.next_turn).push(reply);
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "inbox".into(), answer_for("one"))
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        h.close().await;
    }

    #[tokio::test]
    async fn first_claim_reservation_survives_a_second_claim_until_its_turn_ends() {
        let h = AgentHarness::new(None).await;
        h.continue_question("claimed", &["one"]);
        let first = reply_for("claimed", "first");
        let second = reply_for("claimed", "second");
        dispatch_claimed(&h, first, 1);
        dispatch_claimed(&h, second, 2);
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "claimed".into(), answer_for("one"))
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        h.agent
            .session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        assert!(
            h.service
                .answer(h.agent.clone(), "claimed".into(), answer_for("one"))
                .unwrap()
        );
        h.close().await;
    }

    #[tokio::test]
    async fn unrelated_events_do_not_release_a_reply_reservation() {
        let h = AgentHarness::new(None).await;
        h.continue_question("reserved", &["one"]);
        assert!(
            h.service
                .answer(h.agent.clone(), "reserved".into(), answer_for("one"))
                .unwrap()
        );
        let other_reply = reply_for("other", "other");
        dispatch_claimed(&h, other_reply.clone(), 1);
        dispatch_discarded(&h, other_reply.clone());
        h.agent
            .session
            .append(
                "user/message",
                serde_json::to_value(other_reply).unwrap(),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        h.agent
            .session
            .append("step/end", json!({"turn":1,"step":1}), None)
            .unwrap();
        h.agent
            .session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "reserved".into(), answer_for("one"))
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        h.close().await;
    }

    #[tokio::test]
    async fn claimed_reply_reservation_is_released_by_turn_end() {
        let h = AgentHarness::new(None).await;
        h.continue_question("turn-end", &["one"]);
        assert!(
            h.service
                .answer(h.agent.clone(), "turn-end".into(), answer_for("one"))
                .unwrap()
        );
        let first = lock(&h.agent.inbox.next_step).remove(0);
        dispatch_claimed(&h, first, 1);
        assert_eq!(
            code(
                &h.service
                    .answer(h.agent.clone(), "turn-end".into(), answer_for("one"))
                    .unwrap_err()
            ),
            "REPLY_QUEUED"
        );
        h.agent
            .session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"aborted"}}),
                None,
            )
            .unwrap();
        assert!(
            h.service
                .answer(h.agent.clone(), "turn-end".into(), answer_for("one"))
                .unwrap()
        );
        h.close().await;
    }

    #[tokio::test]
    async fn answer_returns_false_without_projection_composition() {
        let h = AgentHarness::new(None).await;
        *lock(&h.service.inner.projection) = None;
        assert!(
            !h.service
                .answer(
                    h.agent.clone(),
                    "missing".into(),
                    AskUserQuestionAnswer { answers: vec![] }
                )
                .unwrap()
        );
        assert!(lock(&h.agent.inbox.next_step).is_empty());
        h.close().await;
    }

    #[tokio::test]
    async fn answer_refuses_open_and_unknown_calls() {
        let h = AgentHarness::new(None).await;
        h.open_question("open", &["one"]);
        assert!(
            !h.service
                .answer(
                    h.agent.clone(),
                    "open".into(),
                    AskUserQuestionAnswer { answers: vec![] }
                )
                .unwrap()
        );
        assert!(
            !h.service
                .answer(
                    h.agent.clone(),
                    "unknown".into(),
                    AskUserQuestionAnswer { answers: vec![] }
                )
                .unwrap()
        );
        assert!(lock(&h.agent.inbox.next_step).is_empty());
        h.close().await;
    }

    #[tokio::test]
    async fn legacy_interrupted_call_is_never_answerable() {
        let h = AgentHarness::new(None).await;
        h.agent
            .session
            .append(
                "request/header",
                json!({"header":{"tools":[{"name":"ask_user_question","parameters":{"properties":{}}}]}}),
                None,
            )
            .unwrap();
        h.agent
            .session
            .append(
                "tool/call",
                json!({"callId":"legacy","name":"ask_user_question","arguments":"{\"questions\":[{\"id\":\"one\",\"question\":\"One?\"}]}"}),
                None,
            )
            .unwrap();
        assert!(
            !h.service
                .answer(
                    h.agent.clone(),
                    "legacy".into(),
                    AskUserQuestionAnswer { answers: vec![] }
                )
                .unwrap()
        );
        h.close().await;
    }

    #[tokio::test]
    async fn refuses_an_incomplete_or_duplicate_answer_batch() {
        let h = AgentHarness::new(None).await;
        h.continue_question("continued", &["one", "two"]);
        for answers in [
            vec![crate::AskUserQuestionAnswerItem {
                id: "one".into(),
                selected: vec![],
                custom: None,
            }],
            vec![
                crate::AskUserQuestionAnswerItem {
                    id: "one".into(),
                    selected: vec![],
                    custom: None,
                },
                crate::AskUserQuestionAnswerItem {
                    id: "one".into(),
                    selected: vec![],
                    custom: None,
                },
            ],
        ] {
            assert_eq!(
                code(
                    &h.service
                        .answer(
                            h.agent.clone(),
                            "continued".into(),
                            AskUserQuestionAnswer { answers }
                        )
                        .unwrap_err()
                ),
                "BAD_ANSWER"
            );
        }
        h.close().await;
    }

    #[tokio::test]
    async fn rolls_back_the_reply_reservation_when_steering_fails() {
        let h = AgentHarness::new(None).await;
        h.continue_question("continued", &["one"]);
        let answer = AskUserQuestionAnswer {
            answers: vec![crate::AskUserQuestionAnswerItem {
                id: "one".into(),
                selected: vec![],
                custom: None,
            }],
        };
        h.agent.fail_steer.store(true, Ordering::SeqCst);
        assert_eq!(
            h.service
                .answer(h.agent.clone(), "continued".into(), answer.clone())
                .unwrap_err()
                .to_string(),
            "steering failed"
        );
        h.agent.fail_steer.store(false, Ordering::SeqCst);
        assert!(
            h.service
                .answer(h.agent.clone(), "continued".into(), answer)
                .unwrap()
        );
        h.close().await;
    }
}
