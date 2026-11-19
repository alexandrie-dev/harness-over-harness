use std::sync::Arc;

use hoh_agent::{
    AgentOptions, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox, InboxTarget,
    MaintenanceFuture, MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Plugin};
use hoh_llm::{MessageId, ToolCallBlock, ToolCallId, UserMessage};
use hoh_schedule::{
    AtInput, CronInput, DailyInput, DeliveryHistory, MAX_TITLE_LENGTH, Schedule,
    ScheduleCatalogEntry, ScheduleCreateRequest, ScheduleDeliveryReceipt, ScheduleId,
    SchedulePlugin, ScheduleRecord, ScheduleStatus, ScheduleTarget, ScheduleTargetRequest,
    ScheduleTask, ScheduleTiming, WeeklyInput, schedule_domain_spec,
};
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionHeader, SessionId, SessionPlugin,
    Sessions,
};
use hoh_session_persistence::SessionPersistenceService;
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_storage::StoragePlugin;
use hoh_storage_domain::{StorageDomain, StorageDomainPlugin};
use hoh_storage_json::StorageJsonPlugin;
use hoh_system_prompt::{AssembleContext, SystemPrompt, SystemPromptKey};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_schedule::{
    INTERNAL_ERROR_MESSAGE, MANAGEMENT_AUTHORIZATION, SUBAGENT_ERROR_MESSAGE, ToolSchedule,
    model_task, target_help,
};
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    GenericCallView, ToolBatchInput, ToolCallKind, ToolCallView, ToolExecutionPipeline,
    ToolExecutionResult, ToolRestriction, ToolRuntime, Tools, execute_tool_batch,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const DAILY: &str = "DTSTART:20990101T010000Z\nRRULE:FREQ=DAILY";

struct ToolAgent {
    scope: ScopeKey,
    session: Session,
    ctx: Context,
    options: AgentOptions,
}

impl ToolAgent {
    fn new(ctx: &Context, session: Session) -> Self {
        Self {
            scope: ScopeKey::new(),
            session,
            ctx: ctx.clone(),
            options: AgentOptions::default(),
        }
    }
}

impl hoh_tools::Agent for ToolAgent {
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
        panic!("the schedule tool does not read the model")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("the schedule tool does not read the inbox")
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("the schedule tool does not cancel the agent")
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("the schedule tool does not run maintenance")
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("the schedule tool does not send input")
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the schedule tool does not send input")
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the schedule tool does not send input")
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the schedule tool does not send input")
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    agent: Arc<ToolAgent>,
    _root: TempDir,
    _tool: Option<Fiber>,
    schedule: Fiber,
}

impl Harness {
    async fn new() -> Self {
        Self::build(json!({}), true, false).await
    }

    async fn with_schedule_config(schedule_config: Value) -> Self {
        Self::build(schedule_config, true, false).await
    }

    async fn without_tool_mount() -> Self {
        Self::build(json!({}), false, false).await
    }

    async fn with_ended_task() -> Self {
        Self::build(json!({}), true, true).await
    }

    async fn build(schedule_config: Value, mount_tool: bool, seed_ended_task: bool) -> Self {
        let root = TempDir::new().unwrap();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<NativeToolExecutorPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<StoragePlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<StorageJsonPlugin>(
            json!({ "root": root.path().join("storage").to_string_lossy() }),
        )
        .unwrap()
        .settled()
        .await
        .unwrap();
        ctx.plugin::<StorageDomainPlugin>(json!({ "backend": "json" }))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SessionProjectionRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<JsonlSessionPersistencePlugin>(json!({
            "root": root.path().join("sessions").to_string_lossy(),
            "compression": "none",
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();

        let persistence = ctx.get::<SessionPersistenceService>().unwrap();
        for (id, cwd) in [("original", Some("/work")), ("other-session", None)] {
            let mut header = SessionHeader::new(SessionId::from(id), 1);
            header.cwd = cwd.map(str::to_owned);
            let handle = persistence.create(&header, None).await.unwrap();
            handle.flush(None).await.unwrap();
            handle.close().await.unwrap();
        }

        if seed_ended_task {
            let domain = ctx
                .get::<StorageDomain>()
                .unwrap()
                .open(schedule_domain_spec().unwrap())
                .await
                .unwrap();
            let tasks = domain.table::<ScheduleId, ScheduleTask>("tasks").unwrap();
            let id = ScheduleId::from("ended-task");
            tasks
                .put(
                    id.clone(),
                    ScheduleTask {
                        record: ScheduleRecord::After {
                            id,
                            title: "Ended".into(),
                            prompt: "Ended prompt".into(),
                            after_seconds: 1,
                            scheduled_at: "2000-01-01T00:00:01.000Z".into(),
                        },
                        target: ScheduleTarget::Session {
                            session_id: SessionId::from("original"),
                        },
                        involved_session_ids: vec![SessionId::from("original")],
                        status: ScheduleStatus::Ended,
                        created_at: Some("2000-01-01T00:00:00.000Z".into()),
                        updated_at: Some("2000-01-01T00:00:01.000Z".into()),
                        last_delivery: None,
                        delivery_history: DeliveryHistory::default(),
                        pending_delivery: None,
                    },
                )
                .await
                .unwrap();
            domain.close().await.unwrap();
        }

        let session = ctx
            .get::<Sessions>()
            .unwrap()
            .create(
                &ctx,
                Some(SessionId::from("original")),
                CreateSessionOptions {
                    meta: Some(json!({ "cwd": "/work" })),
                    ..CreateSessionOptions::default()
                },
            )
            .unwrap();
        let agent = Arc::new(ToolAgent::new(&ctx, session));

        let schedule = ctx.plugin::<SchedulePlugin>(schedule_config).unwrap();
        schedule.settled().await.unwrap();
        let tool = if mount_tool {
            let tool = ctx.plugin::<ToolSchedule>(json!({})).unwrap();
            tool.settled().await.unwrap();
            Some(tool)
        } else {
            None
        };
        Self {
            kernel,
            ctx,
            agent,
            _root: root,
            _tool: tool,
            schedule,
        }
    }

    async fn call_with_agent(
        &self,
        name: &str,
        arguments: Value,
        agent: Option<Arc<dyn hoh_tools::Agent>>,
    ) -> ToolExecutionResult {
        let control = AbortController::new();
        self.call_with_signal(name, arguments, agent, control.signal())
            .await
    }

    async fn call_with_signal(
        &self,
        name: &str,
        arguments: Value,
        agent: Option<Arc<dyn hoh_tools::Agent>>,
        signal: AbortSignal,
    ) -> ToolExecutionResult {
        let mut input = ToolBatchInput::new(
            vec![ToolCallBlock {
                id: ToolCallId::from("schedule-call"),
                name: name.into(),
                arguments: serde_json::to_string(&arguments).unwrap(),
            }],
            signal,
        );
        input.agent = agent;
        let pipeline = ToolExecutionPipeline::new(self.ctx.clone()).unwrap();
        execute_tool_batch(input, &pipeline, &|| 1)
            .await
            .unwrap()
            .results
            .remove(0)
            .result
    }

    async fn call(&self, name: &str, arguments: Value) -> ToolExecutionResult {
        self.call_with_agent(name, arguments, Some(self.agent.clone()))
            .await
    }

    async fn finish(self) {
        self.kernel.shutdown().await;
    }
}

fn value(result: &ToolExecutionResult) -> &Value {
    result
        .value()
        .unwrap_or_else(|| panic!("tool failed: {result:?}"))
}

#[test]
fn plugin_contract_and_spacing_copy_match_the_source() {
    assert_eq!(ToolSchedule::NAME, "tool-schedule");
    let dependencies: Vec<_> = <hoh_tool_schedule::Deps as hoh_kernel::Inject>::deps()
        .iter()
        .map(|dependency| dependency.name)
        .collect();
    assert_eq!(dependencies, ["tools", "systemPrompt"]);
    assert!(target_help(60).contains("every delivery. New sessions"));
    assert!(target_help(900).contains("every delivery, at most once every 15 minutes"));
    assert!(target_help(90).contains("every delivery, at most once every 90 seconds"));
}

#[tokio::test]
async fn plugin_stays_inert_until_schedule_is_available() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolSchedule>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let tools = ctx.get::<Tools>().unwrap();
    for name in [
        "schedule_create",
        "schedule_list",
        "schedule_view",
        "schedule_edit",
    ] {
        assert!(tools.get(name, None, None).is_none());
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn scoped_mount_visibility_disposal_and_schedule_recovery_match_the_source() {
    let harness = Harness::without_tool_mount().await;
    let key = harness.agent.scope.clone();
    let scope = create_scope(&harness.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let mounted = scope.ctx.plugin::<ToolSchedule>(json!({})).unwrap();
    mounted.settled().await.unwrap();
    let tools = harness.ctx.get::<Tools>().unwrap();
    let names = |scope: Option<&ScopeKey>| {
        let mut names: Vec<_> = tools
            .schemas(scope, None)
            .unwrap()
            .into_iter()
            .map(|schema| schema.name)
            .collect();
        names.sort();
        names
    };
    let expected = vec![
        "schedule_create".to_owned(),
        "schedule_edit".to_owned(),
        "schedule_list".to_owned(),
        "schedule_view".to_owned(),
    ];
    assert_eq!(names(Some(&key)), expected);
    assert!(names(None).is_empty());
    let unrelated = ScopeKey::new();
    assert!(names(Some(&unrelated)).is_empty());
    let prompt = harness.ctx.get::<SystemPromptKey>().unwrap();
    let scoped_prompt = prompt
        .assemble(AssembleContext::new(Some(key.clone())))
        .await
        .unwrap();
    assert_eq!(
        scoped_prompt
            .sections
            .iter()
            .filter(|section| section.text == MANAGEMENT_AUTHORIZATION)
            .count(),
        1
    );
    assert!(
        prompt
            .assemble(AssembleContext::default())
            .await
            .unwrap()
            .sections
            .iter()
            .all(|section| section.text != MANAGEMENT_AUTHORIZATION)
    );

    harness.schedule.dispose().await;
    assert!(names(Some(&key)).is_empty());
    assert!(
        prompt
            .assemble(AssembleContext::new(Some(key.clone())))
            .await
            .unwrap()
            .sections
            .iter()
            .all(|section| section.text != MANAGEMENT_AUTHORIZATION)
    );
    let restored = harness.ctx.plugin::<SchedulePlugin>(json!({})).unwrap();
    restored.settled().await.unwrap();
    assert_eq!(names(Some(&key)), expected);

    scope.dispose().await;
    assert!(names(Some(&key)).is_empty());
    let remount = create_scope(&harness.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    remount
        .ctx
        .plugin::<ToolSchedule>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(names(Some(&key)), expected);
    assert_eq!(
        prompt
            .assemble(AssembleContext::new(Some(key.clone())))
            .await
            .unwrap()
            .sections
            .iter()
            .filter(|section| section.text == MANAGEMENT_AUTHORIZATION)
            .count(),
        1
    );
    remount.dispose().await;
    harness.finish().await;
}

#[tokio::test]
async fn authorization_guidance_follows_visible_mutation_tools_under_restrictions() {
    let harness = Harness::new().await;
    let key = harness.agent.scope.clone();
    let scope = create_scope(&harness.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let tools = harness.ctx.get::<Tools>().unwrap();
    let prompt = harness.ctx.get::<SystemPromptKey>().unwrap();
    for (allow, visible) in [
        (
            vec!["schedule_create", "schedule_list", "schedule_view"],
            true,
        ),
        (
            vec!["schedule_edit", "schedule_list", "schedule_view"],
            true,
        ),
        (vec!["schedule_list", "schedule_view"], false),
    ] {
        let restriction = tools
            .restrict(
                &scope.ctx,
                ToolRestriction {
                    allow: Some(allow.into_iter().map(str::to_owned).collect()),
                    deny: None,
                },
            )
            .unwrap();
        let assembly = prompt
            .assemble(AssembleContext::new(Some(key.clone())))
            .await
            .unwrap();
        assert_eq!(
            assembly
                .sections
                .iter()
                .any(|section| section.text == MANAGEMENT_AUTHORIZATION),
            visible
        );
        restriction.dispose().await;
        assert!(
            prompt
                .assemble(AssembleContext::new(Some(key.clone())))
                .await
                .unwrap()
                .sections
                .iter()
                .any(|section| section.text == MANAGEMENT_AUTHORIZATION)
        );
    }
    scope.dispose().await;
    harness.finish().await;
}

#[tokio::test]
async fn registers_four_exactly_classified_tools_and_authorization() {
    let harness = Harness::new().await;
    let tools = harness.ctx.get::<Tools>().unwrap();
    for name in [
        "schedule_create",
        "schedule_list",
        "schedule_view",
        "schedule_edit",
    ] {
        assert!(tools.get(name, None, None).is_some(), "missing {name}");
    }
    assert!(tools
        .get("schedule_list", None, None)
        .unwrap()
        .is_concurrency_safe
        .as_ref()
        .unwrap()(&json!({})));
    assert!(tools
        .get("schedule_view", None, None)
        .unwrap()
        .is_concurrency_safe
        .as_ref()
        .unwrap()(&json!({ "id": "task" })));
    assert!(
        tools
            .get("schedule_create", None, None)
            .unwrap()
            .description
            .contains("only the latest missed occurrence")
    );
    let schemas = tools.schemas(None, None).unwrap();
    let create_schema = schemas
        .iter()
        .find(|schema| schema.name == "schedule_create")
        .unwrap();
    assert_eq!(
        create_schema.parameters["required"],
        json!(["title", "prompt", "rrule", "target"])
    );
    assert_eq!(
        create_schema.parameters["properties"]["target"]["enum"],
        json!(["this_session", "new_once", "new"])
    );
    assert!(
        create_schema.parameters["properties"]["rrule"]["description"]
            .as_str()
            .unwrap()
            .contains("Example: \"DTSTART")
    );
    let create_definition = tools.get("schedule_create", None, None).unwrap();
    let edit_definition = tools.get("schedule_edit", None, None).unwrap();
    for definition in [&create_definition, &edit_definition] {
        let authoring = serde_json::to_string(&json!({
            "description": definition.description,
            "parameters": definition.parameters,
        }))
        .unwrap();
        assert!(authoring.to_ascii_lowercase().contains("rrule"));
        assert!(!authoring.to_ascii_lowercase().contains("cron"));
        assert!(!authoring.contains(MANAGEMENT_AUTHORIZATION));
    }
    assert_eq!(
        create_definition.output.schema["oneOf"][1]["properties"]["code"]["enum"],
        json!([
            "invalid_prompt",
            "invalid_rule",
            "invalid_time_zone",
            "not_future",
            "frequency_too_high",
            "subagent_session",
            "internal_error",
        ])
    );
    assert_eq!(
        create_definition.output.schema["oneOf"][0]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(
        (create_definition.output.render)(&json!({}), &json!({ "kind": "rrule", "title": "é" }),)
            .unwrap()[0]
            .text_value(),
        Some("{\"kind\":\"rrule\",\"title\":\"é\"}")
    );
    let prompt = harness.ctx.get::<SystemPromptKey>().unwrap();
    let assembly = prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(
        assembly
            .sections
            .iter()
            .find(|section| section.name == "tool:schedule")
            .map(|section| section.text.as_str()),
        Some(MANAGEMENT_AUTHORIZATION)
    );
    harness.finish().await;
}

#[tokio::test]
async fn configured_new_session_spacing_appears_in_the_registered_schema() {
    for (seconds, expected) in [
        (900, "every delivery, at most once every 15 minutes"),
        (90, "every delivery, at most once every 90 seconds"),
    ] {
        let harness = Harness::with_schedule_config(json!({
            "newSessionIntervalSeconds": seconds,
        }))
        .await;
        let tools = harness.ctx.get::<Tools>().unwrap();
        let description = tools
            .get("schedule_create", None, None)
            .unwrap()
            .parameters["properties"]["target"]["description"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(description.contains(expected), "{description}");
        harness.finish().await;
    }
}

#[tokio::test]
async fn create_list_view_update_and_delete_use_host_schedule() {
    let harness = Harness::new().await;
    let created = harness
        .call(
            "schedule_create",
            json!({
                "title": "  Queue check  ",
                "prompt": " Check the queue\n",
                "rrule": DAILY,
                "target": "this_session",
            }),
        )
        .await;
    let created = value(&created).clone();
    assert_eq!(created["kind"], "rrule");
    assert_eq!(created["title"], "Queue check");
    assert_eq!(created["prompt"], "Check the queue");
    assert_eq!(created["rrule"], DAILY);
    assert_eq!(created["scheduledAt"], "2099-01-01T01:00:00.000Z");
    assert_eq!(created["scheduledAtLocal"], "2099-01-01T01:00:00+00:00");
    assert_eq!(
        created["target"],
        json!({ "kind": "session", "sessionId": "original" })
    );
    assert_eq!(created["status"], "active");
    assert!(created.get("createdAt").is_none());
    assert!(created.get("involvedSessionIds").is_none());
    let id = created["id"].as_str().unwrap();

    let listed = harness.call("schedule_list", json!({})).await;
    assert_eq!(value(&listed)["items"], json!([created.clone()]));
    assert_eq!(value(&listed)["shown"], 1);
    assert_eq!(value(&listed)["total"], 1);
    let undeclared_filter = harness
        .call("schedule_list", json!({ "sessionId": "somewhere-else" }))
        .await;
    assert_eq!(value(&undeclared_filter)["total"], 1);

    let viewed = harness.call("schedule_view", json!({ "id": id })).await;
    assert_eq!(value(&viewed), &created);

    let edited = harness
        .call(
            "schedule_edit",
            json!({
                "id": id,
                "action": "update",
                "title": "Renamed",
                "active": false,
                "expectedUpdatedAt": created["updatedAt"],
            }),
        )
        .await;
    assert_eq!(value(&edited)["title"], "Renamed");
    assert_eq!(value(&edited)["status"], "paused");

    let deleted = harness
        .call("schedule_edit", json!({ "id": id, "action": "delete" }))
        .await;
    assert_eq!(value(&deleted), &json!({ "id": id, "deleted": true }));
    let missing = harness.call("schedule_view", json!({ "id": id })).await;
    assert_eq!(
        value(&missing),
        &json!({ "id": id, "code": "schedule_not_found" })
    );
    harness.finish().await;
}

#[tokio::test]
async fn list_pages_filters_and_includes_tasks_from_other_sessions() {
    let harness = Harness::new().await;
    let service = harness.ctx.get::<Schedule>().unwrap();
    let other = service
        .create(
            ScheduleCreateRequest {
                title: "Other session".into(),
                prompt: "Check elsewhere".into(),
                timing: ScheduleTiming::Rrule {
                    rrule: "DTSTART:20990101T003000Z\nRRULE:FREQ=DAILY".into(),
                },
                target: Some(ScheduleTargetRequest::Session {
                    session_id: SessionId::from("other-session"),
                }),
            },
            Some(SessionId::from("other-session")),
            None,
        )
        .await
        .unwrap();
    let other = model_task(&other).unwrap();

    let first = value(
        &harness
            .call(
                "schedule_create",
                json!({ "title": "First", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
            )
            .await,
    )
    .clone();
    let second = value(
        &harness
            .call(
                "schedule_create",
                json!({ "title": "Second", "prompt": "Do it", "rrule": "DTSTART:20990101T020000Z\nRRULE:FREQ=DAILY", "target": "this_session" }),
            )
            .await,
    )
    .clone();
    let third = value(
        &harness
            .call(
                "schedule_create",
                json!({ "title": "Paused", "prompt": "Do it", "rrule": "DTSTART:20990101T030000Z\nRRULE:FREQ=DAILY", "target": "this_session" }),
            )
            .await,
    )
    .clone();
    let paused = value(
        &harness
            .call(
                "schedule_edit",
                json!({ "id": third["id"], "action": "update", "active": false }),
            )
            .await,
    )
    .clone();

    let all = harness.call("schedule_list", json!({})).await;
    assert_eq!(
        value(&all)["items"],
        json!([other.clone(), first.clone(), second, paused.clone()])
    );
    assert_eq!(value(&all)["total"], 4);
    let page = harness
        .call("schedule_list", json!({ "limit": 1, "offset": 1 }))
        .await;
    assert_eq!(value(&page)["items"], json!([first]));
    assert_eq!(value(&page)["shown"], 1);
    assert_eq!(value(&page)["limit"], 1);
    assert_eq!(value(&page)["offset"], 1);
    assert_eq!(value(&page)["total"], 4);
    let active = harness
        .call("schedule_list", json!({ "active": true }))
        .await;
    assert_eq!(value(&active)["total"], 3);
    let inactive = harness
        .call("schedule_list", json!({ "active": false }))
        .await;
    assert_eq!(value(&inactive)["items"], json!([paused]));
    let undeclared = harness
        .call("schedule_list", json!({ "sessionId": "original" }))
        .await;
    assert_eq!(value(&undeclared)["total"], 4);
    let edited_other = harness
        .call(
            "schedule_edit",
            json!({ "id": other["id"], "action": "update", "title": "Renamed here" }),
        )
        .await;
    assert_eq!(value(&edited_other)["title"], "Renamed here");
    assert_eq!(
        value(&edited_other)["target"],
        json!({ "kind": "session", "sessionId": "other-session" })
    );
    assert!(value(&edited_other).get("involvedSessionIds").is_none());
    let raw = service
        .view(hoh_schedule::ScheduleViewRequest {
            id: other["id"].as_str().unwrap().into(),
        })
        .await
        .unwrap();
    let hoh_schedule::ScheduleViewResult::Entry(raw) = raw else {
        panic!("other task disappeared")
    };
    assert_eq!(
        raw.involved_session_ids,
        vec![
            SessionId::from("other-session"),
            SessionId::from("original")
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn exposes_only_declared_input_failures_and_keeps_selector_messages() {
    let harness = Harness::new().await;
    for bounds in [
        json!({ "limit": 0 }),
        json!({ "limit": 501 }),
        json!({ "offset": -1 }),
    ] {
        let invalid_page = harness.call("schedule_list", bounds).await;
        assert_eq!(
            value(&invalid_page),
            &json!({
                "code": "invalid_rule",
                "message": "limit must be 1-500 and offset must be a non-negative integer.",
            })
        );
    }
    let invalid = harness
        .call(
            "schedule_create",
            json!({
                "title": "Queue",
                "prompt": "Do it",
                "rrule": "RRULE:FREQ=DAILY",
                "target": "this_session",
            }),
        )
        .await;
    assert_eq!(
        value(&invalid),
        &json!({
            "code": "invalid_rule",
            "message": "rrule must contain one DTSTART line followed by one RRULE line.",
        })
    );

    let created = harness
        .call(
            "schedule_create",
            json!({ "title": "Queue", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
        )
        .await;
    let id = value(&created)["id"].as_str().unwrap();
    let delete_with_patch = harness
        .call(
            "schedule_edit",
            json!({ "id": id, "action": "delete", "title": "No" }),
        )
        .await;
    assert_eq!(
        value(&delete_with_patch),
        &json!({
            "code": "invalid_selector",
            "message": "delete accepts only id, action, and expectedUpdatedAt.",
        })
    );
    let empty_update = harness
        .call("schedule_edit", json!({ "id": id, "action": "update" }))
        .await;
    assert_eq!(
        value(&empty_update),
        &json!({
            "code": "invalid_selector",
            "message": "update requires at least one of title, prompt, rrule, target, or active.",
        })
    );
    harness.finish().await;
}

#[tokio::test]
async fn edit_updates_all_fields_handles_conflicts_and_reports_missing_ids() {
    let harness = Harness::new().await;
    let created = value(
        &harness
            .call(
                "schedule_create",
                json!({ "title": "Original", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
            )
            .await,
    )
    .clone();
    let id = created["id"].clone();
    let changed_rule = "DTSTART;TZID=Asia/Singapore:20990105T080000\nRRULE:FREQ=WEEKLY;BYDAY=MO";
    let updated = value(
        &harness
            .call(
                "schedule_edit",
                json!({
                    "id": id, "action": "update", "title": "Weekly check",
                    "prompt": "Check weekly", "rrule": changed_rule,
                    "target": "this_session", "active": false,
                    "expectedUpdatedAt": created["updatedAt"],
                }),
            )
            .await,
    )
    .clone();
    assert_eq!(updated["title"], "Weekly check");
    assert_eq!(updated["prompt"], "Check weekly");
    assert_eq!(updated["rrule"], changed_rule);
    assert_eq!(updated["status"], "paused");
    assert_eq!(
        updated["target"],
        json!({ "kind": "session", "sessionId": "original" })
    );

    let resumed = harness
        .call(
            "schedule_edit",
            json!({ "id": id, "action": "update", "active": true }),
        )
        .await;
    assert_eq!(value(&resumed)["status"], "active");

    let stale_update = harness
        .call(
            "schedule_edit",
            json!({
                "id": id, "action": "update", "title": "Stale",
                "expectedUpdatedAt": created["updatedAt"],
            }),
        )
        .await;
    assert_eq!(
        value(&stale_update),
        &json!({ "id": id, "code": "schedule_conflict" })
    );
    let stale_delete = harness
        .call(
            "schedule_edit",
            json!({
                "id": id, "action": "delete",
                "expectedUpdatedAt": created["updatedAt"],
            }),
        )
        .await;
    assert_eq!(
        value(&stale_delete),
        &json!({ "id": id, "code": "schedule_conflict" })
    );

    let blank = harness
        .call(
            "schedule_edit",
            json!({ "id": id, "action": "update", "title": " " }),
        )
        .await;
    assert_eq!(
        value(&blank),
        &json!({
            "code": "invalid_prompt",
            "message": "title is required and must be non-empty after trimming.",
        })
    );
    let read_time_only = harness
        .call(
            "schedule_edit",
            json!({
                "id": id, "action": "update",
                "expectedUpdatedAt": value(&resumed)["updatedAt"],
            }),
        )
        .await;
    assert_eq!(
        value(&read_time_only),
        &json!({
            "code": "invalid_selector",
            "message": "update requires at least one of title, prompt, rrule, target, or active.",
        })
    );

    let deleted = harness
        .call("schedule_edit", json!({ "id": id, "action": "delete" }))
        .await;
    assert_eq!(value(&deleted), &json!({ "id": id, "deleted": true }));
    for action in ["delete", "update"] {
        let args = if action == "delete" {
            json!({ "id": id, "action": action })
        } else {
            json!({ "id": id, "action": action, "title": "Gone" })
        };
        let missing = harness.call("schedule_edit", args).await;
        assert_eq!(
            value(&missing),
            &json!({ "id": id, "code": "schedule_not_found" })
        );
    }
    harness.finish().await;
}

#[tokio::test]
async fn ended_one_shot_requires_a_future_rule_before_it_can_resume() {
    let harness = Harness::with_ended_task().await;
    let viewed = harness
        .call("schedule_view", json!({ "id": "ended-task" }))
        .await;
    assert_eq!(value(&viewed)["kind"], "after");
    assert_eq!(value(&viewed)["status"], "ended");

    let rejected = harness
        .call(
            "schedule_edit",
            json!({ "id": "ended-task", "action": "update", "active": true }),
        )
        .await;
    assert_eq!(
        value(&rejected),
        &json!({
            "code": "not_future",
            "message": "This task has no future occurrence. Set a future schedule before resuming it.",
        })
    );

    let resumed = harness
        .call(
            "schedule_edit",
            json!({
                "id": "ended-task", "action": "update", "rrule": DAILY, "active": true,
            }),
        )
        .await;
    assert_eq!(value(&resumed)["kind"], "rrule");
    assert_eq!(value(&resumed)["status"], "active");
    assert_eq!(value(&resumed)["rrule"], DAILY);
    assert_eq!(value(&resumed)["scheduledAt"], "2099-01-01T01:00:00.000Z");
    harness.finish().await;
}

#[tokio::test]
async fn already_canceled_create_and_edit_are_hidden_and_non_mutating() {
    let harness = Harness::new().await;
    let created = value(
        &harness
            .call(
                "schedule_create",
                json!({ "title": "Original", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
            )
            .await,
    )
    .clone();
    for (name, arguments) in [
        (
            "schedule_create",
            json!({ "title": "Canceled", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
        ),
        (
            "schedule_edit",
            json!({ "id": created["id"], "action": "update", "title": "Canceled" }),
        ),
    ] {
        let controller = AbortController::new();
        controller.abort();
        let result = harness
            .call_with_signal(
                name,
                arguments,
                Some(harness.agent.clone()),
                controller.signal(),
            )
            .await;
        let failure = result.error().expect("cancellation must stop dispatch");
        assert_eq!(failure.message, "tool call aborted before dispatch");
        let info = failure.info.as_ref().expect("coded cancellation");
        assert_eq!(info.name, "AbortError");
        assert_eq!(info.code, "ABORTED_BEFORE_DISPATCH");
    }
    let list = harness.call("schedule_list", json!({})).await;
    assert_eq!(value(&list)["total"], 1);
    assert_eq!(value(&list)["items"][0]["title"], "Original");
    harness.finish().await;
}

#[tokio::test]
async fn create_returns_each_declared_input_error_without_storing_a_task() {
    let cases = [
        (
            json!({ "title": " " }),
            json!({ "code": "invalid_prompt", "message": "title is required and must be non-empty after trimming." }),
        ),
        (
            json!({ "title": "x".repeat(MAX_TITLE_LENGTH + 1) }),
            json!({ "code": "invalid_prompt", "message": format!("title must be at most {MAX_TITLE_LENGTH} characters.") }),
        ),
        (
            json!({ "prompt": "\n" }),
            json!({ "code": "invalid_prompt", "message": "prompt must be non-empty after trimming." }),
        ),
        (
            json!({ "rrule": "RRULE:FREQ=DAILY" }),
            json!({ "code": "invalid_rule", "message": "rrule must contain one DTSTART line followed by one RRULE line." }),
        ),
        (
            json!({ "rrule": "DTSTART:20990101T010000Z\\nRRULE:FREQ=DAILY" }),
            json!({ "code": "invalid_rule", "message": "rrule must contain one DTSTART line followed by one RRULE line." }),
        ),
        (
            json!({ "rrule": "DTSTART;TZID=Mars/Olympus:20990101T080000\nRRULE:FREQ=DAILY" }),
            json!({ "code": "invalid_time_zone", "message": "TZID must be UTC or a valid IANA Area/Location name." }),
        ),
        (
            json!({ "rrule": "DTSTART:20990101T010000Z\nRRULE:FREQ=SECONDLY" }),
            json!({ "code": "frequency_too_high", "message": "FREQ=SECONDLY is not supported; occurrences must be at least 60 seconds apart." }),
        ),
        (
            json!({ "rrule": "DTSTART:20000101T010000Z\nRRULE:FREQ=DAILY;UNTIL=20000110T000000Z" }),
            json!({ "code": "not_future", "message": "rrule must contain an occurrence strictly in the future." }),
        ),
    ];
    for (patch, expected) in cases {
        let harness = Harness::new().await;
        let mut arguments = json!({
            "title": "Queue", "prompt": "Do it", "rrule": DAILY, "target": "this_session",
        });
        arguments
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let result = harness.call("schedule_create", arguments).await;
        assert_eq!(value(&result), &expected);
        assert_eq!(
            harness
                .ctx
                .get::<Schedule>()
                .unwrap()
                .list(Default::default())
                .await
                .unwrap()
                .total,
            0
        );
        harness.finish().await;
    }
}

#[tokio::test]
async fn refuses_missing_and_delegated_callers_without_touching_schedule() {
    let harness = Harness::new().await;
    let calls = [
        (
            "schedule_create",
            json!({ "title": "Queue", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
        ),
        ("schedule_list", json!({})),
        ("schedule_view", json!({ "id": "task-1" })),
        (
            "schedule_edit",
            json!({ "id": "task-1", "action": "delete" }),
        ),
    ];
    for (name, arguments) in &calls {
        let missing = harness.call_with_agent(name, arguments.clone(), None).await;
        assert_eq!(
            value(&missing),
            &json!({ "code": "internal_error", "message": INTERNAL_ERROR_MESSAGE })
        );
    }

    let delegated = Arc::new(ToolAgent::new(
        &harness.ctx,
        Session::create(
            SessionId::from("delegated"),
            None,
            Some(json!({
                "version": 4,
                "id": "delegated",
                "createdAt": 1,
                "isSeeded": false,
                "delegationDepth": 1,
            })),
            None,
            None,
        )
        .unwrap(),
    ));
    for (name, arguments) in calls {
        let refused = harness
            .call_with_agent(name, arguments, Some(delegated.clone()))
            .await;
        assert_eq!(
            value(&refused),
            &json!({ "code": "subagent_session", "message": SUBAGENT_ERROR_MESSAGE })
        );
    }
    assert_eq!(
        harness
            .ctx
            .get::<Schedule>()
            .unwrap()
            .list(Default::default())
            .await
            .unwrap()
            .total,
        0
    );
    harness.finish().await;
}

#[tokio::test]
async fn rejects_model_values_outside_the_declared_target_enum() {
    let harness = Harness::new().await;
    let result = harness
        .call(
            "schedule_create",
            json!({ "title": "Queue", "prompt": "Do it", "rrule": DAILY, "target": "session" }),
        )
        .await;
    assert!(result.error().is_some());
    assert_eq!(
        harness
            .ctx
            .get::<Schedule>()
            .unwrap()
            .list(Default::default())
            .await
            .unwrap()
            .total,
        0
    );
    harness.finish().await;
}

#[tokio::test]
async fn presents_each_call_as_the_source_generic_card() {
    let harness = Harness::new().await;
    let tools = harness.ctx.get::<Tools>().unwrap();
    let cases = [
        (
            "schedule_create",
            json!({ "title": "Queue", "prompt": "Do it", "rrule": DAILY, "target": "this_session" }),
            "Create scheduled task",
            ToolCallKind::Other,
            Some(json!("Queue")),
        ),
        (
            "schedule_list",
            json!({}),
            "List scheduled tasks",
            ToolCallKind::Read,
            None,
        ),
        (
            "schedule_view",
            json!({ "id": "task-1" }),
            "View scheduled task",
            ToolCallKind::Read,
            Some(json!("task-1")),
        ),
        (
            "schedule_edit",
            json!({ "id": "task-1", "action": "update", "title": "Renamed" }),
            "Edit scheduled task",
            ToolCallKind::Other,
            Some(json!("task-1")),
        ),
        (
            "schedule_edit",
            json!({ "id": "task-1", "action": "delete" }),
            "Delete scheduled task",
            ToolCallKind::Other,
            Some(json!("task-1")),
        ),
    ];
    for (name, args, title, kind, raw_input) in cases {
        assert_eq!(
            tools
                .get(name, None, None)
                .unwrap()
                .present_call
                .as_ref()
                .unwrap()(&args),
            Some(ToolCallView::Generic(GenericCallView {
                title: title.into(),
                kind: Some(kind),
                raw_input,
                content: None,
                locations: None,
            }))
        );
    }
    assert!(
        tools
            .get("schedule_create", None, None)
            .unwrap()
            .present_call
            .as_ref()
            .unwrap()(
            &json!({ "title": 42, "prompt": "x", "rrule": DAILY, "target": "this_session" })
        )
        .is_none()
    );
    harness.finish().await;
}

#[tokio::test]
async fn view_list_and_edit_preserve_authoritative_fields_for_all_timing_kinds() {
    let harness = Harness::new().await;
    let service = harness.ctx.get::<Schedule>().unwrap();
    let cases = vec![
        (
            "rrule",
            ScheduleTiming::Rrule {
                rrule: DAILY.into(),
            },
            "rrule",
        ),
        (
            "cron",
            ScheduleTiming::Cron {
                cron: CronInput {
                    expression: "0 1 * * *".into(),
                    time_zone: "UTC".into(),
                },
            },
            "expression",
        ),
        (
            "every",
            ScheduleTiming::Every { every_seconds: 60 },
            "everySeconds",
        ),
        (
            "after",
            ScheduleTiming::After {
                after_seconds: 3600,
            },
            "afterSeconds",
        ),
        (
            "at",
            ScheduleTiming::At {
                at: AtInput::Instant("2099-01-01T01:00:00Z".into()),
            },
            "scheduledAt",
        ),
        (
            "daily",
            ScheduleTiming::Daily {
                daily: DailyInput {
                    time: "01:00:00".into(),
                    time_zone: "UTC".into(),
                },
            },
            "time",
        ),
        (
            "weekly",
            ScheduleTiming::Weekly {
                weekly: WeeklyInput {
                    time: "01:00:00".into(),
                    time_zone: "UTC".into(),
                    weekdays: vec![3],
                },
            },
            "weekdays",
        ),
    ];
    let mut expected_kinds = Vec::new();
    for (kind, timing, authoritative_field) in cases {
        let created = service
            .create(
                ScheduleCreateRequest {
                    title: format!("{kind} task"),
                    prompt: "Do it".into(),
                    timing,
                    target: Some(ScheduleTargetRequest::Session {
                        session_id: SessionId::from("original"),
                    }),
                },
                Some(SessionId::from("original")),
                None,
            )
            .await
            .unwrap();
        let id = created.record.id().as_str();
        let viewed = harness.call("schedule_view", json!({ "id": id })).await;
        assert_eq!(value(&viewed)["kind"], kind);
        assert!(value(&viewed).get(authoritative_field).is_some());
        let edited = harness
            .call(
                "schedule_edit",
                json!({ "id": id, "action": "update", "title": "Renamed" }),
            )
            .await;
        assert_eq!(value(&edited)["kind"], kind);
        assert_eq!(
            value(&edited)[authoritative_field],
            value(&viewed)[authoritative_field]
        );
        expected_kinds.push(kind);
    }
    let listed = harness.call("schedule_list", json!({})).await;
    let mut listed_kinds: Vec<_> = value(&listed)["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    listed_kinds.sort_unstable();
    expected_kinds.sort_unstable();
    assert_eq!(listed_kinds, expected_kinds);
    harness.finish().await;
}

#[test]
fn model_projection_keeps_authoritative_fields_and_hides_host_bookkeeping() {
    let entry = ScheduleCatalogEntry {
        record: ScheduleRecord::Rrule {
            id: "schedule-1".into(),
            title: "Queue".into(),
            prompt: "Check".into(),
            rrule: DAILY.into(),
            scheduled_at: "2099-01-01T01:00:00.000Z".into(),
        },
        target: ScheduleTarget::Session {
            session_id: "original".into(),
        },
        involved_session_ids: vec!["original".into(), "editor".into()],
        status: ScheduleStatus::Active,
        created_at: Some("2026-01-01T00:00:00.000Z".into()),
        updated_at: Some("2026-01-02T00:00:00.000Z".into()),
        execution_active: Some(true),
        delivery_floor_ends_at: Some("2026-01-03T00:00:00.000Z".into()),
        last_delivery: Some(ScheduleDeliveryReceipt {
            scheduled_at: "2026-01-01T01:00:00.000Z".into(),
            delivered_at: "2026-01-01T01:00:01.000Z".into(),
            message_id: MessageId::from("message-1"),
            target_session_id: "original".into(),
        }),
    };
    let projected = model_task(&entry).unwrap();
    assert_eq!(projected["kind"], "rrule");
    assert_eq!(projected["scheduledAtLocal"], "2099-01-01T01:00:00+00:00");
    assert_eq!(projected["updatedAt"], "2026-01-02T00:00:00.000Z");
    assert_eq!(
        projected["lastDelivery"],
        json!({
            "scheduledAt": "2026-01-01T01:00:00.000Z",
            "deliveredAt": "2026-01-01T01:00:01.000Z",
            "targetSessionId": "original",
        })
    );
    for key in [
        "involvedSessionIds",
        "createdAt",
        "executionActive",
        "deliveryFloorEndsAt",
    ] {
        assert!(projected.get(key).is_none(), "leaked {key}");
    }
}

#[test]
fn source_locale_metadata_is_preserved() {
    let locales = [
        ("de", include_str!("../locale/de.json")),
        ("en", include_str!("../locale/en.json")),
        ("es", include_str!("../locale/es.json")),
        ("fr", include_str!("../locale/fr.json")),
        ("ja", include_str!("../locale/ja.json")),
        ("pt", include_str!("../locale/pt.json")),
        ("ru", include_str!("../locale/ru.json")),
        ("zh", include_str!("../locale/zh.json")),
    ];
    for (locale, source) in locales {
        let value: Value = serde_json::from_str(source).unwrap();
        assert!(value["meta"]["title"].as_str().is_some(), "{locale}");
        assert!(value["meta"]["description"].as_str().is_some(), "{locale}");
    }
}
