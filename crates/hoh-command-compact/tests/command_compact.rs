use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, Live, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_command_compact::{COMMAND_NAME, CommandCompact, DEFINITION_ID, DESCRIPTION, USAGE};
use hoh_commands::{CommandId, CommandResult, CommandRuntime, Commands};
use hoh_compaction::{
    Compaction, CompactionAgentContext, CompactionEndEvent, CompactionEngine, CompactionId,
    CompactionResult, CompactionStartEvent, CompactionSummaryEvent, CompactionSummaryOutput,
    CompactionTrigger, ManualCompactAgentContext, ManualCompactionError, ManualCompactionErrorCode,
    ShadowedRange,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Disposer, Fiber, Kernel, Service};
use hoh_llm::{ContentBlock, MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId, SessionSeq};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Semaphore;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    fn new(ctx: &Context) -> Arc<Self> {
        let id = SessionId::from("command-compact-test");
        Arc::new(Self {
            id: id.clone(),
            session: Session::create(id, None, None, None, None).unwrap(),
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

#[derive(Clone)]
enum Outcome {
    Success,
    NoHistory,
    Manual(ManualCompactionErrorCode),
    Unexpected,
    AbortThenManual {
        controller: Arc<AbortController>,
    },
    Gated {
        started: Arc<Semaphore>,
        allow_close: Arc<Semaphore>,
        closed: Arc<Semaphore>,
        allow_flush: Arc<Semaphore>,
        flushed: Arc<Semaphore>,
    },
}

struct Call {
    session: Session,
    provider: Option<String>,
    model: Option<String>,
    signal: AbortSignal,
    source_command_id: Option<hoh_commands::CommandId>,
}

struct StubCompaction {
    outcome: Mutex<Outcome>,
    calls: Mutex<Vec<Call>>,
}

impl StubCompaction {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(Outcome::Success),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, outcome: Outcome) {
        *lock(&self.outcome) = outcome;
    }

    fn append_result(
        session: &Session,
        source_command_id: Option<hoh_commands::CommandId>,
    ) -> Result<CompactionResult, BoxError> {
        let compaction_id = CompactionId::from("command-compact-test");
        let start = session.append(
            CompactionStartEvent::TYPE,
            serde_json::to_value(CompactionStartEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: source_command_id.clone(),
                turn: None,
            })?,
            None,
        )?;
        let shadowed_seqs = vec![
            SessionSeq::new(1).unwrap(),
            SessionSeq::new(3).unwrap(),
            SessionSeq::new(7).unwrap(),
        ];
        let shadowed_range = ShadowedRange {
            start: SessionSeq::new(1).unwrap(),
            end: SessionSeq::new(7).unwrap(),
        };
        let summary = vec![ContentBlock::text("summary")];
        let summary_event = session.append(
            CompactionSummaryEvent::TYPE,
            serde_json::to_value(CompactionSummaryEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: source_command_id.clone(),
                summary: summary.clone(),
                output: CompactionSummaryOutput::other(None),
                shadowed_range,
                shadowed_seqs: shadowed_seqs.clone(),
                shadowed_token_count: 42,
                provider: "provider-test".into(),
                model: "model-test".into(),
                max_tokens: None,
                usage: None,
            })?,
            None,
        )?;
        let end = session.append(
            CompactionEndEvent::TYPE,
            serde_json::to_value(CompactionEndEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: source_command_id.clone(),
                turn: None,
                error: None,
            })?,
            None,
        )?;
        Ok(CompactionResult {
            compaction_id,
            source_command_id,
            start_seq: start.seq(),
            summary_seq: summary_event.seq(),
            end_seq: end.seq(),
            summary,
            shadowed_range,
            shadowed_seqs,
            shadowed_token_count: 42,
        })
    }
}

impl CompactionEngine for StubCompaction {
    fn compact_if_needed<'a>(
        &'a self,
        _: &'a CompactionAgentContext,
        _: CompactionTrigger,
        _: AbortSignal,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>> {
        Box::pin(async { Ok(None) })
    }

    fn compact_now<'a>(
        &'a self,
        agent: &'a ManualCompactAgentContext,
        signal: AbortSignal,
        source_command_id: Option<hoh_commands::CommandId>,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>> {
        let session = agent.session.clone();
        lock(&self.calls).push(Call {
            session: session.clone(),
            provider: agent.options.provider.clone(),
            model: agent.options.model.clone(),
            signal,
            source_command_id: source_command_id.clone(),
        });
        let outcome = lock(&self.outcome).clone();
        Box::pin(async move {
            match outcome {
                Outcome::Success => Ok(Some(Self::append_result(&session, source_command_id)?)),
                Outcome::NoHistory => Ok(None),
                Outcome::Manual(code) => {
                    Err(Box::new(ManualCompactionError::new(code, "backend detail")) as BoxError)
                }
                Outcome::Unexpected => Err(std::io::Error::other("unexpected backend bug").into()),
                Outcome::AbortThenManual { controller } => {
                    controller.abort_with(std::io::Error::other("late operator cancellation"));
                    Err(Box::new(ManualCompactionError::new(
                        ManualCompactionErrorCode::Summary,
                        "late summary failure",
                    )) as BoxError)
                }
                Outcome::Gated {
                    started,
                    allow_close,
                    closed,
                    allow_flush,
                    flushed,
                } => {
                    started.add_permits(1);
                    allow_close.acquire().await.unwrap().forget();
                    closed.add_permits(1);
                    allow_flush.acquire().await.unwrap().forget();
                    flushed.add_permits(1);
                    Err(Box::new(ManualCompactionError::new(
                        ManualCompactionErrorCode::Summary,
                        "late failure",
                    )) as BoxError)
                }
            }
        })
    }

    fn compact_region<'a>(
        &'a self,
        _: SessionSeq,
        _: SessionSeq,
        _: &'a CompactionAgentContext,
        _: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<CompactionResult, BoxError>> {
        Box::pin(async { Err("unused compact_region".into()) })
    }
}

struct Harness {
    kernel: Kernel,
    commands: Service<Commands>,
    compaction: Arc<StubCompaction>,
    _compaction_registration: Disposer,
    plugin: Fiber,
    agent: Arc<dyn Agent>,
}

impl Harness {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<CommandRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let compaction = StubCompaction::new();
        let registration = ctx.provide::<Compaction>(compaction.clone()).await.unwrap();
        let plugin = ctx.plugin::<CommandCompact>(json!({})).unwrap();
        plugin.settled().await.unwrap();
        let agent: Arc<dyn Agent> = StubAgent::new(&ctx);
        Self {
            kernel,
            commands: ctx.get::<Commands>().unwrap(),
            compaction,
            _compaction_registration: registration,
            plugin,
            agent,
        }
    }

    async fn run(
        &self,
        suffix: &str,
        controller: &AbortController,
    ) -> Result<hoh_commands::CommandExecution, BoxError> {
        self.commands
            .execute(
                self.agent.clone(),
                format!("/compact{suffix}"),
                Vec::new(),
                controller.signal(),
            )
            .await?
            .ok_or_else(|| "compact command was not registered".into())
    }

    async fn finish(self) {
        self.kernel.shutdown().await;
    }
}

fn expect_last_lifecycle(session: &Session, args: &str, mut expected_done: Value) -> CommandId {
    let events = session.snapshot_events(None, None);
    let run = events
        .iter()
        .rfind(|event| event.event_type() == "command/run")
        .expect("command/run");
    let done = events.last().expect("command/done");
    assert_eq!(done.event_type(), "command/done");
    let command_id: CommandId =
        serde_json::from_value(run.data()["commandId"].clone()).expect("command id");
    assert_eq!(
        run.data(),
        &json!({
            "commandId": command_id.clone(),
            "name": COMMAND_NAME,
            "args": args,
            "source": {"kind": "user"},
        })
    );
    expected_done
        .as_object_mut()
        .expect("expected command/done object")
        .insert(
            "commandId".into(),
            serde_json::to_value(&command_id).unwrap(),
        );
    assert_eq!(done.data(), &expected_done);
    assert!(session.surface().unwrap().nodes().unwrap().is_empty());
    assert!(session.derive_messages().unwrap().is_empty());
    command_id
}

#[tokio::test]
async fn registers_executes_and_correlates_the_exact_agent_signal_and_command() {
    let harness = Harness::new().await;
    let listed = harness.commands.list(&harness.agent);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, COMMAND_NAME);
    assert_eq!(listed[0].description, DESCRIPTION);
    assert_eq!(
        listed[0].definition_id.as_ref().unwrap().as_str(),
        DEFINITION_ID
    );

    let controller = AbortController::new();
    let execution = harness.run("", &controller).await.unwrap();
    assert_eq!(
        execution.result,
        CommandResult::success_from(
            SessionSeq::new(2).unwrap(),
            Some("Compacted 3 history items (~42 tokens).")
        )
    );
    {
        let calls = lock(&harness.compaction.calls);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session, *harness.agent.session());
        assert_eq!(calls[0].provider.as_deref(), Some("provider-test"));
        assert_eq!(calls[0].model.as_deref(), Some("model-test"));
        assert_eq!(
            calls[0].source_command_id.as_ref(),
            Some(&execution.command_id)
        );
        controller.abort_with("after completion");
        let expected = controller.signal().reason().unwrap();
        assert!(
            calls[0]
                .signal
                .reason()
                .is_some_and(|actual| actual.ptr_eq(&expected))
        );
    }

    let events = harness.agent.session().snapshot_events(None, None);
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        [
            "command/run",
            "compaction/start",
            "compaction/summary",
            "compaction/end",
            "command/done"
        ]
    );
    assert_eq!(
        execution.command_id,
        expect_last_lifecycle(
            harness.agent.session(),
            "",
            json!({
                "kind": "success",
                "text": "Compacted 3 history items (~42 tokens).",
                "sourceEventSeq": 2,
            }),
        )
    );
    harness.finish().await;
}

#[tokio::test]
async fn reports_no_history_and_rejects_arguments_without_calling_the_backend() {
    let harness = Harness::new().await;
    harness.compaction.set(Outcome::NoHistory);
    let controller = AbortController::new();
    let empty = harness.run("", &controller).await.unwrap();
    assert_eq!(
        empty.result,
        CommandResult::success(Some("No compactable history yet."))
    );
    assert_eq!(
        empty.command_id,
        expect_last_lifecycle(
            harness.agent.session(),
            "",
            json!({"kind":"success","text":"No compactable history yet."}),
        )
    );
    let rejected = harness.run(" now", &controller).await.unwrap();
    assert_eq!(rejected.result, CommandResult::error(USAGE));
    assert_eq!(
        rejected.command_id,
        expect_last_lifecycle(
            harness.agent.session(),
            " now",
            json!({"kind":"error","text":USAGE}),
        )
    );
    assert_eq!(lock(&harness.compaction.calls).len(), 1);
    harness.finish().await;
}

#[tokio::test]
async fn maps_every_expected_manual_compaction_failure() {
    let harness = Harness::new().await;
    let cases = [
        (
            ManualCompactionErrorCode::Busy,
            "Compaction is unavailable because this process has an active compaction, or the agent is not idle.",
        ),
        (
            ManualCompactionErrorCode::Cancelled,
            "Compaction cancelled.",
        ),
        (
            ManualCompactionErrorCode::Changed,
            "The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log.",
        ),
        (
            ManualCompactionErrorCode::Summary,
            "Compaction could not produce a useful summary. The attempt is recorded in the session log.",
        ),
        (
            ManualCompactionErrorCode::Commit,
            "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying.",
        ),
        (
            ManualCompactionErrorCode::Persistence,
            "Compaction finished, but the session could not be saved.",
        ),
    ];
    for (code, text) in cases {
        harness.compaction.set(Outcome::Manual(code));
        let execution = harness.run("", &AbortController::new()).await.unwrap();
        assert_eq!(execution.result, CommandResult::error(text));
        assert_eq!(
            execution.command_id,
            expect_last_lifecycle(
                harness.agent.session(),
                "",
                json!({"kind":"error","text":text}),
            )
        );
    }
    harness.finish().await;
}

#[tokio::test]
async fn preserves_unexpected_backend_failures() {
    let harness = Harness::new().await;
    harness.compaction.set(Outcome::Unexpected);
    let error = harness.run("", &AbortController::new()).await.unwrap_err();
    assert_eq!(error.to_string(), "unexpected backend bug");
    expect_last_lifecycle(
        harness.agent.session(),
        "",
        json!({"kind":"error","text":"unexpected backend bug"}),
    );
    harness.finish().await;
}

#[tokio::test]
async fn an_abort_that_races_a_late_summary_failure_keeps_the_abort_reason() {
    let harness = Harness::new().await;
    let controller = Arc::new(AbortController::new());
    harness.compaction.set(Outcome::AbortThenManual {
        controller: controller.clone(),
    });
    let error = harness.run("", &controller).await.unwrap_err();
    let reason = controller.signal().reason().unwrap();
    assert!(SharedError::of(error.as_ref()).is_some_and(|actual| actual.ptr_eq(&reason)));
    expect_last_lifecycle(
        harness.agent.session(),
        "",
        json!({"kind":"error","text":"late operator cancellation"}),
    );
    harness.finish().await;
}

#[tokio::test]
async fn abort_stops_the_wait_but_plugin_disposal_drains_the_backend_operation() {
    let harness = Harness::new().await;
    let started = Arc::new(Semaphore::new(0));
    let allow_close = Arc::new(Semaphore::new(0));
    let closed = Arc::new(Semaphore::new(0));
    let allow_flush = Arc::new(Semaphore::new(0));
    let flushed = Arc::new(Semaphore::new(0));
    harness.compaction.set(Outcome::Gated {
        started: started.clone(),
        allow_close: allow_close.clone(),
        closed: closed.clone(),
        allow_flush: allow_flush.clone(),
        flushed: flushed.clone(),
    });
    let controller = AbortController::new();
    let commands = harness.commands.clone();
    let agent = harness.agent.clone();
    let signal = controller.signal();
    let execution = tokio::spawn(async move {
        commands
            .execute(agent, "/compact", Vec::new(), signal)
            .await
    });
    started.acquire().await.unwrap().forget();
    controller.abort_with("operator cancelled");
    let reason = controller.signal().reason().unwrap();
    let error = execution.await.unwrap().unwrap_err();
    assert!(SharedError::of(error.as_ref()).is_some_and(|actual| actual.ptr_eq(&reason)));
    expect_last_lifecycle(
        harness.agent.session(),
        "",
        json!({"kind":"error","text":"operator cancelled"}),
    );

    let disposed = Arc::new(AtomicBool::new(false));
    let observed = disposed.clone();
    let plugin = harness.plugin.clone();
    let disposal = tokio::spawn(async move {
        plugin.dispose().await;
        observed.store(true, Ordering::Release);
    });
    tokio::task::yield_now().await;
    assert!(
        harness
            .commands
            .find(&harness.agent, COMMAND_NAME)
            .is_none()
    );
    assert!(!disposed.load(Ordering::Acquire));

    allow_close.add_permits(1);
    closed.acquire().await.unwrap().forget();
    tokio::task::yield_now().await;
    assert!(!disposed.load(Ordering::Acquire));

    allow_flush.add_permits(1);
    flushed.acquire().await.unwrap().forget();
    disposal.await.unwrap();
    assert!(disposed.load(Ordering::Acquire));
    harness.finish().await;
}
