use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, Live, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_anonymous_user_id::ANONYMOUS_USER_ID_FILE_NAME;
use hoh_command_feedback::*;
use hoh_commands::{CommandExecution, CommandResult, CommandRuntime, Commands};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Service};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionPlugin, Sessions,
};
use hoh_timeout::AbortController;
use serde_json::json;
use std::{fs, sync::Arc};

const USER_ID: &str = "01234567-89ab-4cde-8f01-23456789abcd";

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("test model is not driven".into())
    }
}

struct EmptyInbox;

impl Inbox for EmptyInbox {
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

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: EmptyInbox,
}

impl StubAgent {
    fn new(ctx: &Context, session: Session) -> Arc<Self> {
        Arc::new(Self {
            id: session.id().clone(),
            session,
            ctx: ctx.clone(),
            scope: ScopeKey::new(),
            options: AgentOptions {
                provider: Some(Live::Value("provider-test".into())),
                model: Some(Live::Value("model-test".into())),
                ..AgentOptions::default()
            },
            model: UnusedModel,
            inbox: EmptyInbox,
        })
    }
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
        Ok(task(AbortController::new().signal()))
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

struct Harness {
    kernel: Kernel,
    ctx: Context,
    commands: Service<Commands>,
    session: Session,
    agent: Arc<dyn Agent>,
    plugin: Fiber,
}

impl Harness {
    async fn new(id: &str) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<CommandRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let plugin = ctx.plugin::<CommandFeedback>(json!({})).unwrap();
        plugin.settled().await.unwrap();
        let session = ctx
            .get::<Sessions>()
            .unwrap()
            .create(
                &ctx,
                Some(SessionId::from(id)),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let agent: Arc<dyn Agent> = StubAgent::new(&ctx, session.clone());
        Self {
            kernel,
            commands: ctx.get::<Commands>().unwrap(),
            ctx,
            session,
            agent,
            plugin,
        }
    }

    async fn run(
        &self,
        suffix: &str,
        controller: &AbortController,
    ) -> Result<CommandExecution, BoxError> {
        self.commands
            .execute(
                self.agent.clone(),
                format!("/feedback{suffix}"),
                Vec::new(),
                controller.signal(),
            )
            .await?
            .ok_or_else(|| "feedback command was not registered".into())
    }

    async fn finish(self) {
        self.kernel.shutdown().await;
    }
}

fn feedback_records(session: &Session) -> Vec<FeedbackRecord> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "feedback/record")
        .map(|event| serde_json::from_value(event.data().clone()).unwrap())
        .collect()
}

fn feedback_texts(session: &Session) -> Vec<Option<String>> {
    feedback_records(session)
        .into_iter()
        .map(|record| record.text)
        .collect()
}

#[tokio::test]
async fn registers_exact_global_command_and_disposes_command_and_service() {
    let harness = Harness::new("registration").await;
    assert_eq!(NAME, "command-feedback");
    let listed = harness.commands.list(&harness.agent);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].definition_id.as_ref().unwrap().as_str(),
        "@deepseek-ai/dsh-command-feedback"
    );
    assert_eq!(listed[0].name, "feedback");
    assert_eq!(listed[0].description, "Record feedback about this session");
    assert_eq!(listed[0].input.as_ref().unwrap().hint, "<text>");
    assert_eq!(
        harness
            .commands
            .find(&harness.agent, "feedback")
            .unwrap()
            .record_input,
        Some(false)
    );
    assert!(harness.ctx.get::<SessionFeedback>().is_some());
    harness.plugin.dispose().await;
    assert!(harness.commands.find(&harness.agent, "feedback").is_none());
    assert!(harness.ctx.get::<SessionFeedback>().is_none());
    harness.finish().await;
}

#[tokio::test]
async fn remote_records_exact_payloads_without_command_bookkeeping() {
    let harness = Harness::new("remote-record").await;
    let remote = harness.ctx.get::<SessionFeedback>().unwrap();
    assert_eq!(
        remote
            .record(SessionFeedbackRecordRequest {
                session_id: harness.session.id().clone(),
                text: Some("  the diff view is unreadable ".into()),
                category: Some(FeedbackCategory::ProductInteraction),
            })
            .await
            .unwrap(),
        SessionFeedbackRecordResult::recorded()
    );
    let eager = remote.record(SessionFeedbackRecordRequest {
        session_id: harness.session.id().clone(),
        text: Some("  recorded before polling  ".into()),
        category: None,
    });
    drop(eager);
    assert_eq!(
        remote
            .record(SessionFeedbackRecordRequest {
                session_id: harness.session.id().clone(),
                text: None,
                category: None,
            })
            .await
            .unwrap(),
        SessionFeedbackRecordResult::recorded()
    );
    assert_eq!(
        harness
            .session
            .snapshot_events(None, None)
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        ["feedback/record", "feedback/record", "feedback/record"]
    );
    assert_eq!(
        feedback_records(&harness.session),
        [
            FeedbackRecord {
                text: Some("the diff view is unreadable".into()),
                category: Some(FeedbackCategory::ProductInteraction),
            },
            FeedbackRecord {
                text: Some("recorded before polling".into()),
                category: None,
            },
            FeedbackRecord::default(),
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn remote_reports_session_not_found_without_appending() {
    let harness = Harness::new("remote-missing").await;
    let missing = SessionId::from("no-such-session");
    assert_eq!(
        harness
            .ctx
            .get::<SessionFeedback>()
            .unwrap()
            .record(SessionFeedbackRecordRequest {
                session_id: missing.clone(),
                text: Some("lost".into()),
                category: None,
            })
            .await
            .unwrap(),
        SessionFeedbackRecordResult::session_not_found(missing)
    );
    assert!(harness.session.snapshot_events(None, None).is_empty());
    harness.finish().await;
}

#[test]
fn remote_wire_literals_are_strict() {
    assert_eq!(
        serde_json::to_value(SessionFeedbackRecordResult::recorded()).unwrap(),
        json!({"ok":true,"value":{"recorded":true}})
    );
    assert_eq!(
        serde_json::to_value(SessionFeedbackRecordResult::session_not_found(
            SessionId::from("missing")
        ))
        .unwrap(),
        json!({"ok":false,"error":{"code":"session-not-found","sessionId":"missing"}})
    );
    for invalid in [
        json!({"ok":false,"value":{"recorded":true}}),
        json!({"ok":true,"error":{"code":"session-not-found","sessionId":"missing"}}),
        json!({"ok":true,"value":{"recorded":false}}),
    ] {
        assert!(serde_json::from_value::<SessionFeedbackRecordResult>(invalid).is_err());
    }
}

#[tokio::test]
async fn command_independent_producer_and_category_taxonomy_match_source() {
    let harness = Harness::new("producer").await;
    record_feedback(
        &harness.session,
        FeedbackRecord {
            text: Some("  recorded outside a command  ".into()),
            category: None,
        },
    )
    .unwrap();
    record_feedback(
        &harness.session,
        FeedbackRecord {
            text: Some(" \n\t ".into()),
            category: Some(FeedbackCategory::ServiceStability),
        },
    )
    .unwrap();
    record_feedback(&harness.session, FeedbackRecord::default()).unwrap();
    assert_eq!(
        feedback_records(&harness.session),
        [
            FeedbackRecord {
                text: Some("recorded outside a command".into()),
                category: None,
            },
            FeedbackRecord {
                text: None,
                category: Some(FeedbackCategory::ServiceStability),
            },
            FeedbackRecord::default(),
        ]
    );
    assert_eq!(
        serde_json::to_value(FEEDBACK_CATEGORIES).unwrap(),
        json!([
            "task-result",
            "instruction-following",
            "product-interaction",
            "service-stability",
            "resource-cost",
            "security-privacy-permission",
            "other"
        ])
    );
    harness.finish().await;
}

#[test]
fn command_behaviour_uses_an_isolated_anonymous_identity() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join(ANONYMOUS_USER_ID_FILE_NAME),
        format!("{USER_ID}\n"),
    )
    .unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "command_behaviour_child", "--ignored"])
        .env("DSH_HOME", home.path())
        .env("HOH_COMMAND_FEEDBACK_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let untouched_home = tempfile::tempdir().unwrap();
    let untouched = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "identity_not_called_child", "--ignored"])
        .env("DSH_HOME", untouched_home.path())
        .env("HOH_COMMAND_FEEDBACK_NO_ID_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        untouched.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&untouched.stdout),
        String::from_utf8_lossy(&untouched.stderr)
    );
    assert!(
        !untouched_home
            .path()
            .join(ANONYMOUS_USER_ID_FILE_NAME)
            .exists()
    );
}

#[tokio::test]
#[ignore = "invoked by command_behaviour_uses_an_isolated_anonymous_identity"]
async fn command_behaviour_child() {
    if std::env::var_os("HOH_COMMAND_FEEDBACK_CHILD").as_deref() != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }

    let accepted = Harness::new("command-accepted").await;
    assert_eq!(
        accepted
            .run(" the diff view is unreadable", &AbortController::new())
            .await
            .unwrap()
            .result,
        CommandResult::success(Some(format!(
            "Feedback recorded for session command-accepted\nAnonymous user: {USER_ID}."
        )))
    );
    assert_eq!(
        accepted
            .session
            .snapshot_events(None, None)
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        ["command/run", "feedback/record", "command/done"]
    );
    let events = accepted.session.snapshot_events(None, None);
    assert!(events[0].data().get("args").is_none());
    assert_eq!(
        feedback_texts(&accepted.session),
        [Some("the diff view is unreadable".into())]
    );
    let json = serde_json::to_string(
        &events
            .iter()
            .map(|event| event.as_value())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(json.matches("the diff view is unreadable").count(), 1);
    for event in events.iter() {
        assert!(event.surface_op().is_none());
        assert!(
            accepted
                .session
                .derive_event_message(event)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        accepted
            .session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .is_empty()
    );
    assert!(accepted.session.derive_messages().unwrap().is_empty());

    let home = std::path::PathBuf::from(std::env::var_os("DSH_HOME").unwrap());
    fs::remove_file(home.join(ANONYMOUS_USER_ID_FILE_NAME)).unwrap();
    accepted.plugin.clone().dispose().await;
    accepted
        .ctx
        .plugin::<CommandFeedback>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(
        accepted
            .run(" after reload", &AbortController::new())
            .await
            .unwrap()
            .result,
        CommandResult::success(Some(format!(
            "Feedback recorded for session command-accepted\nAnonymous user: {USER_ID}."
        )))
    );
    fs::write(
        home.join(ANONYMOUS_USER_ID_FILE_NAME),
        format!("{USER_ID}\n"),
    )
    .unwrap();
    accepted.finish().await;

    let repeated = Harness::new("command-repeated").await;
    repeated
        .run(" /plan felt SLOW\n\ttwice today ", &AbortController::new())
        .await
        .unwrap();
    repeated
        .run(" first", &AbortController::new())
        .await
        .unwrap();
    repeated
        .run(" second", &AbortController::new())
        .await
        .unwrap();
    assert_eq!(
        feedback_texts(&repeated.session),
        [
            Some("/plan felt SLOW\n\ttwice today".into()),
            Some("first".into()),
            Some("second".into()),
        ]
    );
    repeated.finish().await;

    let concurrent = Harness::new("command-concurrent").await;
    let signal = AbortController::new().signal();
    let first = concurrent.commands.execute(
        concurrent.agent.clone(),
        "/feedback first",
        Vec::new(),
        signal.clone(),
    );
    let second = concurrent.commands.execute(
        concurrent.agent.clone(),
        "/feedback second",
        Vec::new(),
        signal,
    );
    let (first, second) = tokio::join!(first, second);
    let expected = CommandResult::success(Some(format!(
        "Feedback recorded for session command-concurrent\nAnonymous user: {USER_ID}."
    )));
    assert_eq!(first.unwrap().unwrap().result, expected);
    assert_eq!(second.unwrap().unwrap().result, expected);
    assert_eq!(
        feedback_texts(&concurrent.session),
        [Some("first".into()), Some("second".into())]
    );
    concurrent.finish().await;

    let rejected = Harness::new("command-rejected").await;
    let expected = CommandResult::error("Feedback text is required. Usage: /feedback <text>");
    assert_eq!(
        rejected
            .run("", &AbortController::new())
            .await
            .unwrap()
            .result,
        expected
    );
    assert_eq!(
        rejected
            .run("   \n\t ", &AbortController::new())
            .await
            .unwrap()
            .result,
        expected
    );
    assert!(feedback_texts(&rejected.session).is_empty());
    let events = rejected.session.snapshot_events(None, None);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == "command/done")
            .map(|event| event.data()["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["error", "error"]
    );
    assert!(
        events
            .iter()
            .filter(|event| event.event_type() == "command/run")
            .all(|event| event.data().get("args").is_none())
    );
    rejected.finish().await;

    let cancelled = Harness::new("command-cancelled").await;
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("user cancelled the command"));
    assert_eq!(
        cancelled
            .commands
            .execute(
                cancelled.agent.clone(),
                "/feedback too late",
                Vec::new(),
                controller.signal(),
            )
            .await
            .unwrap_err()
            .to_string(),
        "user cancelled the command"
    );
    assert!(cancelled.session.snapshot_events(None, None).is_empty());
    cancelled.finish().await;
}

#[tokio::test]
#[ignore = "invoked by command_behaviour_uses_an_isolated_anonymous_identity"]
async fn identity_not_called_child() {
    if std::env::var_os("HOH_COMMAND_FEEDBACK_NO_ID_CHILD").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let harness = Harness::new("identity-not-called").await;
    harness
        .ctx
        .get::<SessionFeedback>()
        .unwrap()
        .record(SessionFeedbackRecordRequest {
            session_id: harness.session.id().clone(),
            text: Some("remote does not need identity".into()),
            category: None,
        })
        .await
        .unwrap();
    assert_eq!(
        harness
            .run("", &AbortController::new())
            .await
            .unwrap()
            .result,
        CommandResult::error("Feedback text is required. Usage: /feedback <text>")
    );
    let home = std::path::PathBuf::from(std::env::var_os("DSH_HOME").unwrap());
    assert!(!home.join(ANONYMOUS_USER_ID_FILE_NAME).exists());
    harness.finish().await;
}
