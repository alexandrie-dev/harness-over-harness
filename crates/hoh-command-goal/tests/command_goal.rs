use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, Agents, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_agent_loop_testkit::{create_inbox_stub, unsupported_model};
use hoh_attachment::{
    AttachmentId, AttachmentStore, Attachments, FileAttachmentRef, ImageAttachmentLimits,
    ImageAttachmentRef, ImageMediaType, SaveImageAttachment, StoredImageAttachment,
};
use hoh_command_goal::{
    COMMAND_NAME, CommandGoal, DEFINITION_ID, DESCRIPTION, INJECT, INPUT_HINT, NAME, USAGE,
};
use hoh_commands::{
    CommandExecution, CommandResult, CommandRuntime, CommandSubmitAttachment, Commands,
};
use hoh_error::BoxError;
use hoh_goal::{
    CreateGoalRequest, GoalActivation, GoalBlockReason, GoalError, GoalPhase, GoalServicePlugin,
    Goals,
};
use hoh_kernel::{Context, Fiber, Kernel, Plugin};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId, SessionPlugin, Sessions};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

async fn mount<P: Plugin>(ctx: &Context) {
    let fiber = ctx.plugin::<P>(json!({})).unwrap();
    fiber.settled().await.unwrap();
}

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
    fail_followup: AtomicBool,
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
        self.model.as_ref()
    }
    fn inbox(&self) -> &dyn Inbox {
        self.inbox.as_ref()
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
    fn send(&self, message: UserMessage, target: InboxTarget, _: bool) -> Result<(), BoxError> {
        self.inbox.append(target, message)
    }
    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        if self.fail_followup.load(Ordering::SeqCst) {
            return Err("unexpected followup failure".into());
        }
        self.inbox.append(InboxTarget::NextTurn, message)
    }
    fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    commands: hoh_kernel::Service<Commands>,
    goals: hoh_kernel::Service<Goals>,
    agent: Arc<StubAgent>,
    agent_dyn: Arc<dyn Agent>,
    plugin: Fiber,
}

impl Harness {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount::<SessionPlugin>(&ctx).await;
        mount::<SessionProjectionRegistry>(&ctx).await;
        mount::<CommandRuntime>(&ctx).await;
        mount::<hoh_agent::AgentRegistryPlugin>(&ctx).await;
        mount::<GoalServicePlugin>(&ctx).await;
        let plugin = ctx.plugin::<CommandGoal>(json!({})).unwrap();
        plugin.settled().await.unwrap();
        let id = SessionId::from(format!(
            "command-goal-{}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let session = ctx
            .get::<Sessions>()
            .unwrap()
            .create(&ctx, Some(id), Default::default())
            .unwrap();
        let agent = Arc::new(StubAgent {
            id: session.id().clone(),
            session,
            ctx: ctx.clone(),
            options: AgentOptions::default(),
            scope: ScopeKey::new(),
            model: unsupported_model(),
            inbox: create_inbox_stub(),
            fail_followup: AtomicBool::new(false),
        });
        let agent_dyn: Arc<dyn Agent> = agent.clone();
        ctx.get::<Agents>()
            .unwrap()
            .register(&ctx, agent_dyn.clone())
            .await
            .unwrap();
        Self {
            kernel,
            commands: ctx.get::<Commands>().unwrap(),
            goals: ctx.get::<Goals>().unwrap(),
            ctx,
            agent,
            agent_dyn,
            plugin,
        }
    }

    async fn run(&self, suffix: &str) -> Result<CommandExecution, BoxError> {
        self.run_with(suffix, Vec::new()).await
    }

    async fn run_with(
        &self,
        suffix: &str,
        attachments: Vec<CommandSubmitAttachment>,
    ) -> Result<CommandExecution, BoxError> {
        self.commands
            .execute(
                self.agent_dyn.clone(),
                format!("/goal{suffix}"),
                attachments,
                AbortController::new().signal(),
            )
            .await?
            .ok_or_else(|| "goal command was not registered".into())
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn text(result: &CommandExecution) -> &str {
    result.result.text().expect("command text")
}

fn goal_change_count(agent: &Arc<StubAgent>) -> usize {
    agent
        .session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "goal/change")
        .count()
}

#[tokio::test]
async fn registers_one_global_command_and_disposes_it() {
    let h = Harness::new().await;
    assert_eq!(NAME, "command-goal");
    assert_eq!(INJECT, ["commands", "goals"]);
    let listed = h.commands.list(&h.agent_dyn);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].definition_id.as_ref().unwrap().as_str(),
        DEFINITION_ID
    );
    assert_eq!(listed[0].name, COMMAND_NAME);
    assert_eq!(listed[0].description, DESCRIPTION);
    assert_eq!(listed[0].input.as_ref().unwrap().hint, INPUT_HINT);
    assert!(listed[0].input.as_ref().unwrap().accepts_attachments());
    h.plugin.dispose().await;
    assert!(h.commands.find(&h.agent_dyn, COMMAND_NAME).is_none());
    h.close().await;
}

#[tokio::test]
async fn shows_empty_status_without_mutating_the_goal_domain() {
    let h = Harness::new().await;
    let result = h.run("").await.unwrap();
    assert_eq!(
        result.result,
        CommandResult::success(Some(format!("No goal is currently set.\n{USAGE}")))
    );
    assert_eq!(goal_change_count(&h.agent), 0);
    h.close().await;
}

#[tokio::test]
async fn creates_trimmed_objective_and_refuses_silent_replacement() {
    let h = Harness::new().await;
    let created = h.run("\n  finish the release  ").await.unwrap();
    assert!(text(&created).contains("Goal created\nStatus: active"));
    assert!(text(&created).contains("Objective: finish the release"));
    assert!(text(&created).contains("Rounds: 0/256"));
    assert!(text(&created).contains("Activation: armed"));
    assert_eq!(goal_change_count(&h.agent), 1);
    let refused = h.run(" replacement").await.unwrap();
    assert_eq!(
        refused.result,
        CommandResult::error(
            "A goal is already active. Use /goal edit <objective> to change it or /goal clear before replacing it."
        )
    );
    assert_eq!(goal_change_count(&h.agent), 1);
    h.close().await;
}

#[tokio::test]
async fn treats_only_exact_control_words_as_controls() {
    let h = Harness::new().await;
    h.run(" pause everything only after verification")
        .await
        .unwrap();
    assert_eq!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().goal.objective,
        "pause everything only after verification"
    );
    h.run(" edit\u{feff}replacement objective").await.unwrap();
    assert_eq!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().goal.objective,
        "replacement objective"
    );
    h.close().await;

    let h = Harness::new().await;
    h.run(" edit\u{0085}literal objective").await.unwrap();
    assert_eq!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().goal.objective,
        "edit\u{0085}literal objective"
    );
    h.close().await;
}

#[tokio::test]
async fn edits_inline_requires_objective_and_replaces_a_complete_goal() {
    let empty = Harness::new().await;
    assert!(text(&empty.run(" edit").await.unwrap()).contains("requires a replacement objective"));
    assert!(
        text(&empty.run(" edit replacement").await.unwrap()).contains("/goal edit requires one")
    );
    empty.close().await;

    let h = Harness::new().await;
    h.run(" first").await.unwrap();
    let first = h.goals.get(&h.agent_dyn).unwrap().unwrap();
    assert!(text(&h.run(" EDIT\n  second  ").await.unwrap()).contains("Goal updated"));
    let second = h.goals.get(&h.agent_dyn).unwrap().unwrap();
    assert_eq!(second.goal.id, first.goal.id);
    assert_eq!(second.goal.objective, "second");
    assert_eq!(second.goal.revision, 2);
    h.goals.complete(&h.agent_dyn, &second.as_ref()).unwrap();
    assert!(text(&h.run(" edit third").await.unwrap()).contains("Goal created"));
    let third = h.goals.get(&h.agent_dyn).unwrap().unwrap();
    assert_ne!(third.goal.id, first.goal.id);
    assert_eq!(third.goal.objective, "third");
    assert_eq!(third.goal.revision, 1);
    h.close().await;
}

#[tokio::test]
async fn returns_direct_missing_state_results_for_pause_resume_and_clear() {
    let h = Harness::new().await;
    assert!(text(&h.run(" pause").await.unwrap()).contains("/goal pause requires one"));
    assert!(text(&h.run(" resume").await.unwrap()).contains("/goal resume requires one"));
    assert_eq!(
        h.run(" clear").await.unwrap().result,
        CommandResult::success(Some("No goal to clear."))
    );
    h.close().await;
}

#[tokio::test]
async fn pauses_resumes_clears_and_converts_expected_domain_rejections() {
    let h = Harness::new().await;
    h.run(" work").await.unwrap();
    assert_eq!(
        h.run(" RESUME").await.unwrap().result,
        CommandResult::error(
            "The goal command is not valid for the current state. Run /goal to view available commands."
        )
    );
    assert!(text(&h.run(" PAUSE").await.unwrap()).contains("Goal paused"));
    let paused = h.goals.get(&h.agent_dyn).unwrap().unwrap();
    assert_eq!(paused.goal.phase, GoalPhase::Paused);
    assert_eq!(paused.activation, GoalActivation::Disarmed);
    assert!(text(&h.run(" resume").await.unwrap()).contains("Goal resumed"));
    assert_eq!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().activation,
        GoalActivation::Armed
    );
    assert_eq!(
        h.run(" clear").await.unwrap().result,
        CommandResult::success(Some("Goal cleared."))
    );
    assert!(h.goals.get(&h.agent_dyn).unwrap().is_none());
    h.close().await;
}

#[tokio::test]
async fn shows_every_durable_phase_and_disarmed_active_state() {
    let h = Harness::new().await;
    let mut goal = h
        .goals
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "state matrix".into(),
                max_goal_rounds: Some(1),
            },
        )
        .unwrap();
    h.goals.disarm(&h.agent_dyn).unwrap();
    let active = h.run("").await.unwrap();
    assert!(
        text(&active)
            .contains("Status: active\nObjective: state matrix\nRounds: 0/1\nActivation: disarmed")
    );
    assert!(text(&active).contains("/goal resume"));
    goal = h.goals.resume(&h.agent_dyn, &goal.as_ref()).unwrap();
    goal = h.goals.pause(&h.agent_dyn, &goal.as_ref()).unwrap();
    assert!(text(&h.run("").await.unwrap()).contains("Status: paused"));
    goal = h.goals.resume(&h.agent_dyn, &goal.as_ref()).unwrap();
    goal = h
        .goals
        .block(
            &h.agent_dyn,
            &goal.as_ref(),
            GoalBlockReason {
                code: "upstream-unavailable".into(),
                message: "Provider unavailable".into(),
            },
        )
        .unwrap();
    let blocked = h.run("").await.unwrap();
    assert!(text(&blocked).contains("Status: blocked"));
    assert!(text(&blocked).contains("Blocker: upstream-unavailable: Provider unavailable"));
    goal = h.goals.resume(&h.agent_dyn, &goal.as_ref()).unwrap();
    h.goals.complete(&h.agent_dyn, &goal.as_ref()).unwrap();
    let complete = h.run("").await.unwrap();
    assert!(text(&complete).contains("Status: complete"));
    assert!(text(&complete).contains("Commands: /goal <objective>, /goal clear"));
    h.close().await;
}

#[tokio::test]
async fn unexpected_followup_failures_are_not_converted_to_command_results() {
    let h = Harness::new().await;
    provide_attachments(&h).await;
    h.agent.fail_followup.store(true, Ordering::SeqCst);
    let error = h
        .run_with(" objective", vec![image()])
        .await
        .expect_err("unexpected failure must escape");
    assert_eq!(error.to_string(), "unexpected followup failure");
    h.close().await;
}

#[tokio::test]
async fn corrupt_goal_state_is_not_converted_to_the_generic_invalid_result() {
    let h = Harness::new().await;
    h.agent
        .session
        .append("goal/change", json!({ "kind": "broken" }), None)
        .unwrap();
    let error = h.run("").await.expect_err("corrupt state must escape");
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert!(error.is_named(GoalError::NAME));
    assert_eq!(error.code(), "GOAL_STATE_CORRUPT");
    h.close().await;
}

#[derive(Debug)]
struct TestStore {
    limits: ImageAttachmentLimits,
}

impl TestStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 1024,
                max_images_per_message: 4,
                max_message_image_bytes: 4096,
                max_image_pixels: 1_000_000,
                max_image_dimension: 1024,
                media_types: vec![ImageMediaType::Png],
            },
        })
    }
}

impl AttachmentStore for TestStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }
    fn validate_image<'a>(
        &'a self,
        _: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async move {
            Ok(ImageAttachmentRef {
                attachment_id: AttachmentId::from("sha256:image"),
                media_type: input.media_type,
                bytes: input.data.len() as u64,
                width: 1,
                height: 1,
                name: input.name.clone(),
                original_dimensions: None,
            })
        })
    }
    fn read_image<'a>(
        &'a self,
        _: &'a ImageAttachmentRef,
        _: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err("unused".into()) })
    }
}

fn image() -> CommandSubmitAttachment {
    CommandSubmitAttachment::Image {
        media_type: ImageMediaType::Png,
        data: "AAAA".into(),
        name: Some("ref.png".into()),
    }
}

async fn provide_attachments(h: &Harness) {
    h.ctx
        .provide::<Attachments>(TestStore::new())
        .await
        .unwrap();
    h.commands
        .register_file_receipt_resolver(Arc::new(|_, receipt| {
            (receipt == "receipt-notes").then(|| FileAttachmentRef {
                attachment_id: AttachmentId::from("sha256:file"),
                bytes: 5,
                name: "notes.txt".into(),
            })
        }))
        .unwrap();
}

fn mixed() -> Vec<CommandSubmitAttachment> {
    vec![
        image(),
        CommandSubmitAttachment::File {
            receipt_id: "receipt-notes".into(),
        },
    ]
}

#[tokio::test]
async fn submits_one_user_followup_with_ordered_mixed_attachments() {
    let h = Harness::new().await;
    provide_attachments(&h).await;
    let result = h.run_with(" rebuild the cathedral", mixed()).await.unwrap();
    assert_eq!(result.result.kind(), "success");
    let followups = h.agent.inbox.next_turn();
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].source.kind, "user");
    assert_eq!(
        followups[0]
            .content
            .iter()
            .map(|block| block.r#type.as_str())
            .collect::<Vec<_>>(),
        ["image", "file", "text"]
    );
    assert_eq!(
        followups[0].content[0]
            .image()
            .unwrap()
            .attachment
            .name
            .as_deref(),
        Some("ref.png")
    );
    assert_eq!(
        followups[0].content[1].file().unwrap().attachment.name,
        "notes.txt"
    );
    assert_eq!(
        followups[0].content[2].text_value(),
        Some("Reference attachments for the goal objective.")
    );
    h.close().await;
}

#[tokio::test]
async fn attachments_accompany_edit_and_post_complete_recreate() {
    let h = Harness::new().await;
    provide_attachments(&h).await;
    let created = h
        .goals
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "initial objective".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.run_with(" edit refined objective", vec![image()])
        .await
        .unwrap();
    assert_eq!(h.agent.inbox.next_turn().len(), 1);
    let edited = h.goals.get(&h.agent_dyn).unwrap().unwrap();
    assert_eq!(edited.goal.id, created.goal.id);
    h.goals.complete(&h.agent_dyn, &edited.as_ref()).unwrap();
    h.run_with(" edit replacement objective", vec![image()])
        .await
        .unwrap();
    assert_eq!(h.agent.inbox.next_turn().len(), 2);
    assert_ne!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().goal.id,
        created.goal.id
    );
    h.close().await;
}

#[tokio::test]
async fn rejects_attachments_on_non_objective_subcommands_without_mutation() {
    let h = Harness::new().await;
    provide_attachments(&h).await;
    h.goals
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "active objective".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let changes = goal_change_count(&h.agent);
    for suffix in [" pause", "", " clear"] {
        assert_eq!(
            h.run_with(suffix, vec![image()]).await.unwrap().result,
            CommandResult::error(
                "Attachments only accompany a goal objective: /goal <objective> or /goal edit <objective>."
            )
        );
    }
    assert_eq!(goal_change_count(&h.agent), changes);
    assert!(h.agent.inbox.next_turn().is_empty());
    assert_eq!(
        h.goals.get(&h.agent_dyn).unwrap().unwrap().goal.phase,
        GoalPhase::Active
    );
    h.close().await;
}

#[tokio::test]
async fn does_not_submit_attachments_when_creation_is_refused() {
    let h = Harness::new().await;
    provide_attachments(&h).await;
    h.goals
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "existing objective".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let result = h
        .run_with(" replacement objective", vec![image()])
        .await
        .unwrap();
    assert_eq!(result.result.kind(), "error");
    assert!(h.agent.inbox.next_turn().is_empty());
    h.close().await;
}
