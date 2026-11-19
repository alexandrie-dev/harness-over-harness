//! Port of inbox.spec.ts.
use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentInboxDiscarded, AgentInboxInserted, AgentOptions, AgentStatus, CancelOptions,
    Inbox, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection, agent_events,
};
use hoh_agent_loop::{ReactLoopInbox, inbox_projection_definition};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionForkSource, SessionId, SessionStore,
};
use hoh_session_projection::{ProjectionHandle, SessionProjectionRegistry};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct UnusedModel;
impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture does not drive the Agent".into())
    }
}

struct UnsupportedInbox;
impl Inbox for UnsupportedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn next_step(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn clear(&self) -> Result<(), BoxError> {
        Err("this test Agent does not support Inbox mutations".into())
    }
    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        self.clear()
    }
    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        self.clear()
    }
    fn replace(&self, _: &hoh_llm::MessageId, _: UserMessage) -> Result<bool, BoxError> {
        self.clear().map(|()| false)
    }
    fn remove(&self, _: &hoh_llm::MessageId) -> Result<bool, BoxError> {
        self.clear().map(|()| false)
    }
    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        self.clear().map(|()| vec![])
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: UnusedModel,
    inbox: UnsupportedInbox,
}

impl Agent for StubAgent {
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
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(hoh_timeout::AbortController::new().signal()))
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

fn stub_agent(ctx: &Context, id: &str, session: Session) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: SessionId::from(id),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: UnusedModel,
        inbox: UnsupportedInbox,
    })
}

fn pending(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

struct Fixture {
    _store: SessionStore,
    _kernel: Kernel,
    ctx: Context,
    session: Session,
    inbox: ReactLoopInbox,
}

async fn projection(ctx: &Context) -> Arc<ProjectionHandle<hoh_agent::InboxState>> {
    Arc::new(
        SessionProjectionRegistry::new(ctx)
            .expect("registry")
            .register(ctx, inbox_projection_definition())
            .await
            .expect("registers"),
    )
}

fn make_inbox(
    ctx: &Context,
    handle: &Arc<ProjectionHandle<hoh_agent::InboxState>>,
    id: &str,
    session: &Session,
) -> ReactLoopInbox {
    let agent = stub_agent(ctx, id, session.clone());
    let (read, reader_session) = (handle.clone(), session.clone());
    let dispatch_ctx = ctx.clone();
    ReactLoopInbox::new(
        Arc::new(move || read.state_of(&reader_session)),
        session.clone(),
        Arc::new(move || Some(agent_events(&dispatch_ctx, &agent))),
    )
}

async fn fixture(id: &str) -> Fixture {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let handle = projection(&ctx).await;
    let store = SessionStore::new();
    let session = store
        .create(
            &ctx,
            Some(SessionId::from(id)),
            CreateSessionOptions::default(),
        )
        .expect("session");
    let inbox = make_inbox(&ctx, &handle, id, &session);
    Fixture {
        _store: store,
        _kernel: kernel,
        ctx,
        session,
        inbox,
    }
}

fn texts(messages: &[UserMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| {
            m.content
                .first()
                .and_then(ContentBlock::text_value)
                .map(str::to_owned)
        })
        .collect()
}

#[tokio::test]
async fn replaces_a_pending_message_by_identity_across_both_lists() {
    let f = fixture("replace-inbox").await;
    let inserted = Arc::new(Mutex::new(Vec::<UserMessage>::new()));
    let discarded = Arc::new(Mutex::new(Vec::<UserMessage>::new()));
    let sink = inserted.clone();
    f.ctx
        .on_contained::<AgentInboxInserted>(move |e| {
            sink.lock().unwrap().push(e.message.clone());
            Ok(())
        })
        .unwrap();
    let sink = discarded.clone();
    f.ctx
        .on_contained::<AgentInboxDiscarded>(move |e| {
            sink.lock().unwrap().push(e.message.clone());
            Ok(())
        })
        .unwrap();
    let original = pending("original");
    let next_step = pending("step");
    let replacement = pending("replacement");
    let mut edited_step = next_step.clone();
    edited_step.content = vec![ContentBlock::text("edited step")];
    f.inbox
        .append(InboxTarget::NextTurn, original.clone())
        .unwrap();
    f.inbox
        .append(InboxTarget::NextStep, next_step.clone())
        .unwrap();

    assert!(
        !f.inbox
            .replace(&pending("missing").id, replacement.clone())
            .unwrap()
    );
    assert!(f.inbox.replace(&original.id, replacement.clone()).unwrap());
    assert!(f.inbox.replace(&next_step.id, edited_step.clone()).unwrap());
    assert_eq!(f.inbox.next_turn(), std::slice::from_ref(&replacement));
    assert_eq!(f.inbox.next_step(), [edited_step.clone()]);
    assert_eq!(
        *discarded.lock().unwrap(),
        [original.clone(), next_step.clone()]
    );
    assert_eq!(
        *inserted.lock().unwrap(),
        [
            original,
            next_step,
            replacement.clone(),
            edited_step.clone()
        ]
    );
    let error = f
        .inbox
        .replace(&edited_step.id, replacement.clone())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("message \"{}\" is already pending", replacement.id)
    );
}

#[tokio::test]
async fn normalizes_splice_coordinates_rejects_duplicate_identities_and_reports_missing_removals() {
    let f = fixture("splice-inbox").await;
    let (first, second, prefixed) = (pending("first"), pending("second"), pending("prefixed"));

    f.inbox
        .splice(
            InboxTarget::NextTurn,
            f64::NAN,
            f64::NAN,
            vec![first.clone(), second.clone()],
        )
        .unwrap();
    assert_eq!(f.inbox.next_turn(), [first.clone(), second.clone()]);
    assert_eq!(
        f.inbox
            .splice(InboxTarget::NextTurn, -1.0, 1.0, vec![])
            .unwrap(),
        std::slice::from_ref(&second)
    );
    f.inbox
        .prepend(InboxTarget::NextTurn, prefixed.clone())
        .unwrap();
    assert_eq!(f.inbox.next_turn(), [prefixed, first.clone()]);
    assert!(!f.inbox.remove(&second.id).unwrap());
    let error = f
        .inbox
        .append(InboxTarget::NextStep, first.clone())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("message \"{}\" is already pending", first.id)
    );
}

#[tokio::test]
async fn clears_both_pending_lists_as_durable_cancellations() {
    let f = fixture("clear-inbox").await;
    let discarded = Arc::new(Mutex::new(Vec::<UserMessage>::new()));
    let sink = discarded.clone();
    f.ctx
        .on_contained::<AgentInboxDiscarded>(move |e| {
            sink.lock().unwrap().push(e.message.clone());
            Ok(())
        })
        .unwrap();
    let (next_turn, next_step) = (pending("turn"), pending("step"));
    f.inbox
        .append(InboxTarget::NextTurn, next_turn.clone())
        .unwrap();
    f.inbox
        .append(InboxTarget::NextStep, next_step.clone())
        .unwrap();
    let before_clear = f.session.snapshot_events(None, None).len();

    f.inbox.clear().unwrap();

    assert!(f.inbox.next_turn().is_empty());
    assert!(f.inbox.next_step().is_empty());
    assert_eq!(*discarded.lock().unwrap(), [next_step, next_turn]);
    let data: Vec<Value> = f.session.snapshot_events(None, None)[before_clear..]
        .iter()
        .map(|event| event.data().clone())
        .collect();
    assert_eq!(
        data,
        [
            json!({"target": "next-step", "start": 0, "removedCount": 1, "inserted": [], "outcome": "canceled"}),
            json!({"target": "next-turn", "start": 0, "removedCount": 1, "inserted": [], "outcome": "canceled"}),
        ]
    );
    f.inbox.clear().unwrap();
    assert_eq!(
        f.session.snapshot_events(None, None).len(),
        before_clear + 2
    );
}

#[tokio::test]
async fn updates_the_projection_before_session_observers_run() {
    let f = fixture("inbox-live-projection").await;
    let message = pending("direct");
    let observed = Arc::new(Mutex::new(None::<Vec<UserMessage>>));
    // The projection is a lazy fold over the committed log, so the observer sees the event.
    let (sink, handle_inbox) = (observed.clone(), Arc::new(f.inbox));
    let reader = handle_inbox.clone();
    f.ctx
        .on_contained::<hoh_session::SessionEventEmitted>(move |emitted| {
            if emitted.event.event_type() == "agent/inbox/spliced" {
                *sink.lock().unwrap() = Some(reader.next_turn());
            }
            Ok(())
        })
        .unwrap();

    handle_inbox
        .append(InboxTarget::NextTurn, message.clone())
        .unwrap();

    assert_eq!(observed.lock().unwrap().clone(), Some(vec![message]));
}

#[tokio::test]
async fn projects_inherited_inbox_events_in_a_forked_session() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let handle = projection(&ctx).await;
    let store = SessionStore::new();
    let parent = store
        .create(
            &ctx,
            Some(SessionId::from("inbox-fork-parent")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let parent_inbox = make_inbox(&ctx, &handle, "inbox-fork-parent", &parent);
    let inherited = pending("parent pending");
    parent_inbox
        .append(InboxTarget::NextTurn, inherited.clone())
        .unwrap();
    let child = store
        .fork(
            &ctx,
            SessionForkSource::Session(parent.clone()),
            None,
            Some(SessionId::from("inbox-fork-child")),
        )
        .unwrap();
    let child_inbox = make_inbox(&ctx, &handle, "inbox-fork-child", &child);

    assert_eq!(
        child.inherited_event_count().get(),
        parent.snapshot_events(None, None).len() as u64
    );
    assert_eq!(child_inbox.next_turn(), std::slice::from_ref(&inherited));
    let own = pending("child pending");
    child_inbox
        .append(InboxTarget::NextTurn, own.clone())
        .unwrap();
    assert_eq!(texts(&child_inbox.next_turn()), texts(&[inherited, own]));
}

#[tokio::test]
async fn rejects_invalid_durable_coordinates_and_duplicate_identities_during_reconstruction() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let handle = projection(&ctx).await;

    let session = Session::create(
        SessionId::from("invalid-inbox-range"),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    session
        .append(
            "agent/inbox/spliced",
            json!({"target": "next-turn", "start": 0, "removedCount": 1, "inserted": []}),
            None,
        )
        .unwrap();
    let error = handle.state_of(&session).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid persisted inbox splice at session seq 0: invalid inbox splice"
    );

    let message = pending("duplicate");
    let session = Session::create(
        SessionId::from("invalid-inbox-duplicate"),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    for target in ["next-turn", "next-step"] {
        session
            .append(
                "agent/inbox/spliced",
                json!({"target": target, "start": 0, "inserted": [message]}),
                None,
            )
            .unwrap();
    }
    let error = handle.state_of(&session).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "invalid persisted inbox splice at session seq 1: message \"{}\" is already pending",
            message.id
        )
    );
}
