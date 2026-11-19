use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentPreStep, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection, PreStepDecision,
    agent_events,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{
    ContentBlock, MessageId, MessageSource, NewUserMessage, UserMessage, create_user_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, Session, SessionId, SessionLogOffset, SessionSeq, SessionSeqCursor,
    SurfaceIntent,
};
use hoh_session_projection::{
    ProjectionCheckpointRow, SessionProjectionRegistry, SessionProjections,
};
use hoh_time_context::timestamp::{create_timestamp_formatter, format_timestamp};
use hoh_time_context::{Config, NAME, TimeContext, TimeContextError};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::{Value, json};

const BASE: i64 = 1_783_987_200_000;

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("unused model".into())
    }
}

struct UnusedInbox;

impl Inbox for UnusedInbox {
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
    cancel_controller: Option<Arc<AbortController>>,
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
        &UnusedModel
    }

    fn inbox(&self) -> &dyn Inbox {
        &UnusedInbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        if let Some(controller) = &self.cancel_controller {
            controller.abort();
        }
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
        panic!("time-context appends through the pre-step decision")
    }
}

struct Stack {
    kernel: Kernel,
    ctx: Context,
    fiber: Fiber,
}

async fn mount(config: Value) -> Result<Stack, BoxError> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))?
        .settled()
        .await?;
    ctx.plugin::<AgentRegistryPlugin>(json!({}))?
        .settled()
        .await?;
    let fiber = ctx.plugin::<TimeContext>(config)?;
    fiber.settled().await?;
    Ok(Stack { kernel, ctx, fiber })
}

fn session(id: &str) -> Session {
    Session::create(id.into(), None, None, None, None).unwrap()
}

fn agent(ctx: &Context, session: &Session, id: &str) -> Arc<dyn Agent> {
    Arc::new(TestAgent {
        id: id.into(),
        session: session.clone(),
        ctx: ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
        cancel_controller: None,
    })
}

fn cancellable_agent(
    ctx: &Context,
    session: &Session,
    id: &str,
    cancel_controller: Arc<AbortController>,
) -> Arc<dyn Agent> {
    Arc::new(TestAgent {
        id: id.into(),
        session: session.clone(),
        ctx: ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
        cancel_controller: Some(cancel_controller),
    })
}

fn browser_message(text: &str, time_zone: Option<&str>) -> UserMessage {
    let mut source = MessageSource::new("user");
    if let Some(time_zone) = time_zone {
        source
            .data
            .insert("rpcId".into(), json!(format!("rpc-{time_zone}")));
        source
            .data
            .insert("clientTimeZone".into(), json!(time_zone));
    }
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source,
    })
}

fn append_user(session: &Session, message: UserMessage) {
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

fn open_message_turn(session: &Session, turn: u64, time_zone: Option<&str>) {
    session
        .append("turn/start", json!({ "turn": turn }), None)
        .unwrap();
    append_user(session, browser_message(&format!("turn {turn}"), time_zone));
}

async fn decide(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    turn: u64,
    step: u64,
    signal: AbortSignal,
) -> Result<PreStepDecision, BoxError> {
    let proposed = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("request proposal")],
        source: MessageSource::new("time-context-test"),
    });
    agent_events(ctx, &agent)
        .waterfall::<AgentPreStep, PreStepDecision, _>(
            move |agent| AgentPreStep {
                agent,
                messages: vec![proposed.clone()],
                turn,
                step,
                signal,
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            move |event| async move {
                Ok(PreStepDecision::Enter {
                    messages: event.messages.clone(),
                    starts_request_series: false,
                })
            },
        )
        .await
}

async fn fire(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    turn: u64,
    step: u64,
    signal: AbortSignal,
) -> Result<Vec<UserMessage>, BoxError> {
    let decision = decide(ctx, agent.clone(), turn, step, signal).await?;
    let PreStepDecision::Enter { messages, .. } = decision else {
        return Ok(Vec::new());
    };
    let added: Vec<_> = messages
        .into_iter()
        .filter(|message| message.source.kind == NAME)
        .collect();
    for message in &added {
        append_user(agent.session(), message.clone());
    }
    Ok(added)
}

fn context_texts(session: &Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "user/message")
        .filter_map(|event| serde_json::from_value::<UserMessage>(event.data().clone()).ok())
        .filter(|message| message.source.kind == NAME)
        .filter_map(|message| {
            message
                .content
                .iter()
                .find_map(ContentBlock::text_value)
                .map(str::to_owned)
        })
        .collect()
}

fn checkpoint_value(stack: &Stack, session: &Session) -> Value {
    stack
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .checkpoint(session)
        .unwrap()["timeContext"]
        .val
        .clone()
        .unwrap()
}

#[test]
fn formats_exact_iso_shaped_timestamps_in_named_zones() {
    let utc = create_timestamp_formatter(Some("UTC")).unwrap();
    assert_eq!(
        format_timestamp(BASE, &utc, "UTC").unwrap(),
        "2026-07-14T00:00:00+00:00[UTC]"
    );
    let shanghai = create_timestamp_formatter(Some("Asia/Shanghai")).unwrap();
    assert_eq!(
        format_timestamp(BASE + 90_061_000, &shanghai, "Asia/Shanghai").unwrap(),
        "2026-07-15T09:01:01+08:00[Asia/Shanghai]"
    );
    let new_york = create_timestamp_formatter(Some("America/New_York")).unwrap();
    assert_eq!(
        format_timestamp(BASE, &new_york, "America/New_York").unwrap(),
        "2026-07-13T20:00:00-04:00[America/New_York]"
    );
}

#[tokio::test]
async fn records_turn_step_zoned_time_and_snapshot_source() {
    let stack = mount(json!({ "timeZone": "Asia/Shanghai" })).await.unwrap();
    let session = session("first");
    open_message_turn(&session, 1, Some("Asia/Shanghai"));
    let messages = fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "first"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    let source = &messages[0].source;
    assert_eq!(source.kind, NAME);
    assert_eq!(source.data["form"], "snapshot");
    assert_eq!(source.data["sections"][0]["name"], NAME);
    assert_eq!(
        source.data["sections"][0]["text"],
        context_texts(&session)[0]
    );
    assert!(
        context_texts(&session)[0].contains("Browser time zone for this request: Asia/Shanghai.")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_unavailable_first_step_baseline_without_a_visible_message() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let session = session("unavailable");
    session
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "unavailable"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(
        context_texts(&session)[0]
            .contains("Elapsed since the preceding model-visible message: unavailable.")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn uses_the_preceding_durable_step_context_after_step_one() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("later-step");
    let owner = agent(&stack.ctx, &session, "later-step");
    open_message_turn(&session, 3, None);
    fire(
        &stack.ctx,
        owner.clone(),
        3,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    fire(&stack.ctx, owner, 3, 2, AbortController::new().signal())
        .await
        .unwrap();
    let texts = context_texts(&session);
    assert_eq!(texts.len(), 2);
    assert!(texts[1].contains("Elapsed since the preceding step context: 0s."));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn formats_in_one_browser_zone_and_falls_back_for_mixed_zones() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let resolved = session("resolved");
    open_message_turn(&resolved, 1, Some("America/New_York"));
    fire(
        &stack.ctx,
        agent(&stack.ctx, &resolved, "resolved"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&resolved)[0].contains("[America/New_York]"));

    let mixed = session("mixed");
    open_message_turn(&mixed, 1, Some("Asia/Shanghai"));
    append_user(&mixed, browser_message("steer", Some("America/New_York")));
    fire(
        &stack.ctx,
        agent(&stack.ctx, &mixed, "mixed"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    let text = &context_texts(&mixed)[0];
    assert!(text.contains("+00:00[UTC]"));
    assert!(text.contains("mixed [\"America/New_York\",\"Asia/Shanghai\"]"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn later_step_baseline_is_unavailable_at_a_matching_turn_boundary() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let session = session("boundary");
    open_message_turn(&session, 4, None);
    fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "boundary"),
        4,
        2,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(
        context_texts(&session)[0]
            .contains("Elapsed since the preceding step context: unavailable.")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn later_step_baseline_is_unavailable_when_history_is_exhausted() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let session = session("exhausted");
    fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "exhausted"),
        1,
        2,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(
        context_texts(&session)[0]
            .ends_with("Elapsed since the preceding step context: unavailable.")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn applies_positive_interval_across_turns_without_sharing_sessions() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 1000 }))
        .await
        .unwrap();
    let first = session("interval-first");
    let first_agent = agent(&stack.ctx, &first, "interval-first");
    open_message_turn(&first, 1, None);
    fire(
        &stack.ctx,
        first_agent.clone(),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    first
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    open_message_turn(&first, 2, None);
    let skipped = fire(
        &stack.ctx,
        first_agent,
        2,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(skipped.is_empty());

    let independent = session("interval-independent");
    open_message_turn(&independent, 1, None);
    assert_eq!(
        fire(
            &stack.ctx,
            agent(&stack.ctx, &independent, "interval-independent"),
            1,
            1,
            AbortController::new().signal(),
        )
        .await
        .unwrap()
        .len(),
        1
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn durable_shadowed_injection_still_throttles_after_session_resume() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let original = session("shadow-source");
    open_message_turn(&original, 1, None);
    fire(
        &stack.ctx,
        agent(&stack.ctx, &original, "shadow-source"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    let events = original.snapshot_events(None, None);
    let user = events
        .iter()
        .find(|event| {
            event.event_type() == "user/message" && event.data()["source"]["kind"] == "user"
        })
        .unwrap();
    let reading = events
        .iter()
        .find(|event| {
            event.event_type() == "user/message" && event.data()["source"]["kind"] == NAME
        })
        .unwrap();
    let summary = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("compacted history")],
        source: MessageSource::new("compaction-basic"),
    });
    original
        .append(
            "user/message",
            serde_json::to_value(summary).unwrap(),
            Some(SurfaceIntent::replace(
                user.seq(),
                reading.seq(),
                vec![user.seq(), reading.seq()],
            )),
        )
        .unwrap();
    original
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();

    let persisted = original.snapshot_events(None, None);
    let resumed =
        Session::create("shadow-resumed".into(), Some(&persisted), None, None, None).unwrap();
    open_message_turn(&resumed, 2, None);
    let before = resumed.snapshot_events(None, None).len();
    let added = fire(
        &stack.ctx,
        agent(&stack.ctx, &resumed, "shadow-resumed"),
        2,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(added.is_empty());
    assert_eq!(resumed.snapshot_events(None, None).len(), before);
    assert_eq!(context_texts(&resumed).len(), 1);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn skips_an_already_aborted_prompt_submission() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("aborted");
    open_message_turn(&session, 1, None);
    let abort = AbortController::new();
    abort.abort();
    let messages = fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "aborted"),
        1,
        1,
        abort.signal(),
    )
    .await
    .unwrap();
    assert!(messages.is_empty());
    assert!(context_texts(&session).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_invalid_explicit_zone_and_refresh_intervals_at_load() {
    let invalid_zone = match mount(json!({ "timeZone": "Not/A_Real_Zone" })).await {
        Ok(_) => panic!("invalid time zone unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(invalid_zone.to_string().contains("invalid IANA timeZone"));
    for value in [json!(-1), json!(0.5), json!(9_007_199_254_740_992_u64)] {
        let error = match mount(json!({ "refreshIntervalMs": value })).await {
            Ok(_) => panic!("invalid refresh interval unexpectedly loaded"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("refreshIntervalMs must be a non-negative safe integer")
        );
    }
}

#[tokio::test]
async fn removes_its_listener_when_the_plugin_fiber_disposes() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("dispose");
    let owner = agent(&stack.ctx, &session, "dispose");
    open_message_turn(&session, 1, None);
    fire(
        &stack.ctx,
        owner.clone(),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    stack.fiber.dispose().await;
    assert!(
        fire(&stack.ctx, owner, 1, 2, AbortController::new().signal(),)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(context_texts(&session).len(), 1);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn projection_restores_browser_zones_from_a_json_checkpoint_and_suffix() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let projections = stack.ctx.get::<SessionProjections>().unwrap();
    let session = session("checkpoint");
    open_message_turn(&session, 1, Some("Asia/Shanghai"));
    let cut = session.seq();
    let rows = projections.checkpoint(&session).unwrap();
    append_user(&session, browser_message("steer", Some("America/New_York")));
    let suffix = session.snapshot_events(Some(cut), None);
    let restored = projections
        .restore(
            &serde_json::from_value(serde_json::to_value(&rows).unwrap()).unwrap(),
            &suffix,
            cut,
            &serde_json::from_value(session.header().as_ref().clone()).unwrap(),
            session.inherited_event_count(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&restored.checkpoint["timeContext"]).unwrap(),
        serde_json::to_value(&projections.checkpoint(&session).unwrap()["timeContext"]).unwrap()
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_invalid_reported_zones_until_an_eligible_request_validates_them() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 600000 }))
        .await
        .unwrap();
    let session = session("refusal-timing");
    let owner = agent(&stack.ctx, &session, "refusal-timing");
    open_message_turn(&session, 1, Some("UTC"));
    fire(
        &stack.ctx,
        owner.clone(),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    append_user(&session, browser_message("invalid", Some("+08:00")));
    assert_eq!(
        checkpoint_value(&stack, &session)["browserTimeZones"],
        json!(["UTC", "+08:00"])
    );
    assert!(
        fire(
            &stack.ctx,
            owner.clone(),
            1,
            2,
            AbortController::new().signal(),
        )
        .await
        .unwrap()
        .is_empty()
    );
    stack
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .checkpoint(&session)
        .unwrap();
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn rejected_request_defers_invalid_zone_error_until_next_eligible_request() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("refusal-reject");
    let owner = agent(&stack.ctx, &session, "refusal-reject");
    open_message_turn(&session, 1, Some("UTC"));
    fire(
        &stack.ctx,
        owner.clone(),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    append_user(&session, browser_message("invalid", Some("+08:00")));
    let reject = stack
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|_, _| async {
            Ok(PreStepDecision::Reject)
        })
        .unwrap();
    assert_eq!(
        decide(
            &stack.ctx,
            owner.clone(),
            1,
            2,
            AbortController::new().signal(),
        )
        .await
        .unwrap(),
        PreStepDecision::Reject,
    );
    reject.dispose().await;
    let error = decide(&stack.ctx, owner, 1, 2, AbortController::new().signal())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "browser time zone must be canonical UTC or IANA Area/Location: \"+08:00\"",
    );
    assert_eq!(context_texts(&session).len(), 1);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn historical_requested_turn_scan_and_next_turn_zone_reset_are_preserved() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("historical");
    append_user(&session, browser_message("before", Some("Europe/London")));
    assert_eq!(
        checkpoint_value(&stack, &session)["browserTimeZones"],
        json!([])
    );
    open_message_turn(&session, 1, Some("Asia/Shanghai"));
    append_user(
        &session,
        browser_message("duplicate", Some("Asia/Shanghai")),
    );
    open_message_turn(&session, 2, Some("America/New_York"));
    assert_eq!(
        checkpoint_value(&stack, &session)["browserTimeZones"],
        json!(["America/New_York"])
    );
    fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "historical"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(
        context_texts(&session)
            .last()
            .unwrap()
            .contains("mixed [\"America/New_York\",\"Asia/Shanghai\"]")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn projection_clears_open_turn_injection_on_turn_boundaries() {
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let active = session("turn-boundaries");
    active
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    let mut source = MessageSource::new(NAME);
    source.data.insert("form".into(), json!("snapshot"));
    append_user(
        &active,
        create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("reading")],
            source,
        }),
    );
    assert!(checkpoint_value(&stack, &active)["lastTurnInjectionTime"].is_number());
    active
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    assert!(checkpoint_value(&stack, &active)["lastTurnInjectionTime"].is_null());
    active
        .append("turn/start", json!({ "turn": 2 }), None)
        .unwrap();
    assert!(checkpoint_value(&stack, &active)["lastTurnInjectionTime"].is_null());

    let ended = session("ended-first");
    ended
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    assert!(checkpoint_value(&stack, &ended)["lastTurnInjectionTime"].is_null());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn downstream_rejection_and_failure_commit_no_reading() {
    for mode in ["reject", "fail"] {
        let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
            .await
            .unwrap();
        let session = session(&format!("downstream-{mode}"));
        open_message_turn(&session, 1, None);
        let disposer = if mode == "reject" {
            stack
                .ctx
                .on_waterfall::<AgentPreStep, PreStepDecision, _>(|_, _| async {
                    Ok(PreStepDecision::Reject)
                })
                .unwrap()
        } else {
            stack
                .ctx
                .on_waterfall::<AgentPreStep, PreStepDecision, _>(|_, _| async {
                    Err("later pre-step failure".into())
                })
                .unwrap()
        };
        let result = decide(
            &stack.ctx,
            agent(&stack.ctx, &session, mode),
            1,
            1,
            AbortController::new().signal(),
        )
        .await;
        if mode == "reject" {
            assert_eq!(result.unwrap(), PreStepDecision::Reject);
        } else {
            assert!(result.is_err());
        }
        drop(disposer);
        assert!(context_texts(&session).is_empty());
        stack.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn downstream_cancellation_commits_no_reading() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let session = session("downstream-cancel");
    open_message_turn(&session, 1, None);
    let controller = Arc::new(AbortController::new());
    let owner = cancellable_agent(
        &stack.ctx,
        &session,
        "downstream-cancel",
        Arc::clone(&controller),
    );
    let cancel = stack
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            event
                .agent
                .cancel(AgentCancelCause::User, CancelOptions::default())?;
            next.run().await
        })
        .unwrap();
    let result = decide(&stack.ctx, owner, 1, 1, controller.signal())
        .await
        .unwrap();
    assert!(matches!(result, PreStepDecision::Enter { .. }));
    assert!(controller.signal().aborted());
    assert!(context_texts(&session).is_empty());
    drop(cancel);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn namespace_metadata_and_default_config_boot_the_listener() {
    assert_eq!(<TimeContext as hoh_kernel::Plugin>::NAME, NAME);
    let config = Config::default();
    assert!(config.time_zone.is_none());
    assert!(config.refresh_interval_ms.is_none());
    let stack = mount(json!({ "timeZone": "UTC" })).await.unwrap();
    let session = session("loader");
    open_message_turn(&session, 1, None);
    fire(
        &stack.ctx,
        agent(&stack.ctx, &session, "loader"),
        1,
        1,
        AbortController::new().signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session)[0].starts_with("Time sampled while preparing turn 1, step 1:"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn old_checkpoint_replays_full_journal_and_freezes_after_plugin_unload() {
    let stack = mount(json!({ "timeZone": "UTC", "refreshIntervalMs": 0 }))
        .await
        .unwrap();
    let projections = stack.ctx.get::<SessionProjections>().unwrap();
    let source = session("old-row-source");
    open_message_turn(&source, 1, Some("Asia/Shanghai"));
    let expected = projections.checkpoint(&source).unwrap();
    let mut old = expected.clone();
    old.insert(
        "timeContext".into(),
        ProjectionCheckpointRow {
            ver: 2,
            seq: expected["timeContext"].seq,
            val: Some(json!({
                "lastMessageTime": BASE,
                "lastInjectionTime": null,
                "lastTurnInjectionTime": null,
            })),
        },
    );
    let events = source.snapshot_events(None, None);
    let cold = Session::create("old-row-cold".into(), Some(&events), None, None, None).unwrap();
    projections
        .hydrate(&cold, &old, &events, SessionLogOffset::new(0).unwrap())
        .unwrap();
    let rebuilt = projections.checkpoint(&cold).unwrap();
    assert_eq!(rebuilt["timeContext"].ver, 3);
    assert_eq!(
        rebuilt["timeContext"].seq,
        SessionSeqCursor::Seq(SessionSeq::new(cold.seq().get() - 1).unwrap()),
    );
    assert_eq!(rebuilt["timeContext"].val, expected["timeContext"].val);

    let frozen = serde_json::to_value(&rebuilt["timeContext"]).unwrap();
    stack.fiber.dispose().await;
    open_message_turn(&cold, 2, Some("UTC"));
    assert_eq!(
        serde_json::to_value(&projections.checkpoint(&cold).unwrap()["timeContext"]).unwrap(),
        frozen,
    );
    assert!(
        !projections
            .checkpoint(&session("after-unload"))
            .unwrap()
            .contains_key("timeContext")
    );
    stack.kernel.shutdown().await;
}

#[test]
fn system_zone_formatter_resolves_or_reports_unavailability_at_creation() {
    match create_timestamp_formatter(None) {
        Ok(formatter) => assert!(!formatter.time_zone().is_empty()),
        Err(error) => assert!(!error.to_string().is_empty()),
    }
    assert!(create_timestamp_formatter(Some("CET")).is_ok());
    assert!(create_timestamp_formatter(Some("GMT")).is_ok());
    assert_eq!(
        create_timestamp_formatter(Some("Asia/Calcutta"))
            .unwrap()
            .time_zone(),
        "Asia/Kolkata"
    );
    let invalid = create_timestamp_formatter(Some("Not/A_Real_Zone")).unwrap_err();
    assert_eq!(
        invalid.downcast_ref::<TimeContextError>().unwrap().name(),
        "RangeError"
    );
}

#[test]
fn session_log_offsets_used_by_restore_remain_safe() {
    assert_eq!(SessionLogOffset::new(0).unwrap().get(), 0);
}
