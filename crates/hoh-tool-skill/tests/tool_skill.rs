use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use futures_util::{FutureExt, future::BoxFuture};
use hoh_agent::{
    Agent, AgentOptions, AgentPreStep, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection, PreStepDecision,
};
use hoh_agent_loop_testkit::{
    provide_working_directory_fixture, unsupported_inbox, unsupported_model,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause as SessionCancelCause, Session, SessionId, SurfaceIntent};
use hoh_skill::{
    NativeError, SkillCandidate, SkillDefinition, SkillLookupOptions, SkillProvider,
    SkillProviderControl, SkillProviderObservation, SkillProviderOutput, SkillRegistration,
    SkillRegistry, Skills,
};
use hoh_skill_filesystem::SkillFileSystemPlugin;
use hoh_timeout::AbortController;
use hoh_tool_skill::{Config, NAME, SkillCatalogSource, ToolSkillPlugin};
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolExecutionInput, ToolRegistrationOptions, ToolRuntime,
    Tools, define_tool,
};
use serde_json::{Value, json};
use tempfile::TempDir;

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
}

impl StubAgent {
    fn boxed(ctx: &Context, name: &str) -> Arc<dyn Agent> {
        let id = SessionId::from(name);
        Arc::new(Self {
            id: id.clone(),
            session: Session::create(id, None, None, None, None).unwrap(),
            ctx: ctx.clone(),
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: unsupported_model(),
            inbox: unsupported_inbox(),
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
    fn cancel(&self, _: SessionCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
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
        panic!("catalog must be returned from the pre-step boundary")
    }
}

async fn setup(config: Value) -> (Kernel, Context, TempDir, Arc<dyn Agent>) {
    let home = TempDir::new().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    provide_working_directory_fixture(&ctx, Some(home.path().to_str().unwrap()))
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolSkillPlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let agent = StubAgent::boxed(&ctx, "tool-skill-test");
    (kernel, ctx, home, agent)
}

fn register_skill(ctx: &Context, name: &str, description: &str, content: &str, invocation: Value) {
    ctx.get::<Skills>()
        .unwrap()
        .register(
            ctx,
            SkillRegistration::from_value(json!({
                "name":name,
                "description":description,
                "content":content,
                "source":"runtime",
                "provider":"runtime",
                "invocation":invocation,
            })),
        )
        .unwrap();
}

struct ProbeProvider {
    failing: AtomicBool,
    complete: AtomicBool,
    list_calls: AtomicUsize,
    get_calls: AtomicUsize,
    seen_signal: Mutex<Option<hoh_timeout::AbortSignal>>,
    seen_cwd: Mutex<Option<Option<String>>>,
    candidate: Option<SkillCandidate>,
    loaded: Mutex<Option<SkillDefinition>>,
}

impl ProbeProvider {
    fn empty_failing() -> Self {
        Self {
            failing: AtomicBool::new(true),
            complete: AtomicBool::new(true),
            list_calls: AtomicUsize::new(0),
            get_calls: AtomicUsize::new(0),
            seen_signal: Mutex::new(None),
            seen_cwd: Mutex::new(None),
            candidate: None,
            loaded: Mutex::new(None),
        }
    }
}

impl SkillProvider for ProbeProvider {
    fn name(&self) -> &str {
        "probe"
    }

    fn list(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        *self.seen_signal.lock().unwrap() = options.signal.clone();
        *self.seen_cwd.lock().unwrap() = Some(options.cwd.clone());
        if self.failing.load(Ordering::SeqCst) {
            return futures_util::future::ready(Err(Box::new(NativeError::Error(
                "temporarily unavailable".into(),
            )) as BoxError))
            .boxed();
        }
        let candidates = self.candidate.clone().into_iter().collect::<Vec<_>>();
        let output = if self.complete.load(Ordering::SeqCst) {
            SkillProviderOutput::Candidates(candidates)
        } else {
            SkillProviderOutput::Observation(SkillProviderObservation {
                candidates,
                complete: false,
            })
        };
        futures_util::future::ready(Ok(output)).boxed()
    }

    fn get(
        &self,
        _: SkillCandidate,
        _: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        futures_util::future::ready(Ok(self.loaded.lock().unwrap().clone())).boxed()
    }
}

fn register_probe(ctx: &Context, provider: Arc<ProbeProvider>) -> SkillProviderControl {
    let mut control = None;
    ctx.get::<Skills>()
        .unwrap()
        .register_provider(ctx, |given| {
            control = Some(given);
            Ok(provider)
        })
        .unwrap();
    control.unwrap()
}

async fn pre_step(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    claimed: Vec<UserMessage>,
) -> Result<PreStepDecision, BoxError> {
    ctx.waterfall(
        AgentPreStep {
            agent,
            messages: claimed.clone(),
            turn: 1,
            step: 1,
            signal: AbortController::new().signal(),
            model_selection: ModelSelection {
                provider: "mock".into(),
                model: "mock".into(),
                reasoning_effort: None,
            },
        },
        move |_| async move {
            Ok(PreStepDecision::Enter {
                messages: claimed,
                starts_request_series: false,
            })
        },
    )
    .await
}

fn user(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

fn enter(decision: PreStepDecision) -> Vec<UserMessage> {
    let PreStepDecision::Enter { messages, .. } = decision else {
        panic!("step rejected")
    };
    messages
}

fn catalog(messages: &[UserMessage]) -> Option<&UserMessage> {
    messages
        .iter()
        .find(|message| message.source.kind == SkillCatalogSource::KIND)
}

#[tokio::test]
async fn registers_exact_tool_schema_presentation_and_disposes_it() {
    assert_eq!(ToolSkillPlugin::NAME, NAME);
    let home = TempDir::new().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    provide_working_directory_fixture(&ctx, Some(home.path().to_str().unwrap()))
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    register_skill(
        &ctx,
        "lifecycle-skill",
        "Lifecycle",
        "Body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let fiber = ctx.plugin::<ToolSkillPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    let tool = ctx
        .get::<Tools>()
        .unwrap()
        .get("skill", None, None)
        .unwrap();
    assert_eq!(tool.name, "skill");
    assert_eq!(tool.parameters["required"], json!(["name"]));
    let view = tool.present_call.as_ref().unwrap()(&json!({"name":"project-skill"}));
    assert_eq!(
        serde_json::to_value(view).unwrap(),
        json!({"card":"generic","title":"Load skill project-skill","kind":"read","rawInput":"project-skill"})
    );
    let agent = StubAgent::boxed(&ctx, "tool-skill-lifecycle");
    let active = enter(
        pre_step(&ctx, agent.clone(), vec![user("/lifecycle-skill")])
            .await
            .unwrap(),
    );
    assert!(catalog(&active).is_some());
    assert!(
        active
            .iter()
            .any(|message| message.source.kind == "skill-invocation")
    );
    fiber.dispose().await;
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("skill", None, None)
            .is_none()
    );
    let disposed = enter(
        pre_step(&ctx, agent.clone(), vec![user("/lifecycle-skill")])
            .await
            .unwrap(),
    );
    assert_eq!(disposed.len(), 1);
    assert!(catalog(&disposed).is_none());
    assert!(
        disposed
            .iter()
            .all(|message| message.source.kind != "skill-invocation")
    );
    let restarted = ctx.plugin::<ToolSkillPlugin>(json!({})).unwrap();
    restarted.settled().await.unwrap();
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("skill", None, None)
            .is_some()
    );
    let restored = enter(
        pre_step(&ctx, agent, vec![user("/lifecycle-skill")])
            .await
            .unwrap(),
    );
    assert!(catalog(&restored).is_some());
    assert!(
        restored
            .iter()
            .any(|message| message.source.kind == "skill-invocation")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn injects_stable_durable_catalog_and_deduplicates_visible_history() {
    let (kernel, ctx, _home, agent) = setup(json!({"catalogDescriptionMaxLength":50})).await;
    for registration in [
        json!({
            "name":"z-skill","description":"Long   description ".repeat(5),
            "whenToUse":"Never render this routing hint.","source":"secret-source",
            "provider":"runtime","resourceBase":{"kind":"directory","path":"/secret/path"},
            "content":"Secret body.","invocation":{"modelInvocable":true,"userInvocable":true}
        }),
        json!({
            "name":"a-skill","description":"Use {{placeholder}} <safely> & carefully.",
            "source":"runtime","provider":"runtime","content":"A body.",
            "invocation":{"modelInvocable":true,"userInvocable":true}
        }),
        json!({
            "name":"model-only-skill","description":"Model-only skill.",
            "source":"runtime","provider":"runtime","content":"Model-only body.",
            "invocation":{"modelInvocable":true,"userInvocable":false}
        }),
        json!({
            "name":"user-only-skill","description":"User-only skill.",
            "source":"runtime","provider":"runtime","content":"User-only body.",
            "invocation":{"modelInvocable":false,"userInvocable":true}
        }),
    ] {
        ctx.get::<Skills>()
            .unwrap()
            .register(&ctx, SkillRegistration::from_value(registration))
            .unwrap();
    }
    let later = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("later contribution")],
        source: MessageSource::new("later-contribution"),
    });
    let messages = enter(
        pre_step(&ctx, agent.clone(), vec![later.clone()])
            .await
            .unwrap(),
    );
    assert_eq!(messages[0].id, later.id);
    let catalog_message = catalog(&messages).unwrap();
    assert_eq!(
        catalog_message.source.data,
        json!({
            "form":"catalog",
            "entries":[
                {"name":"a-skill","description":"Use {{placeholder}} <safely> & carefully."},
                {"name":"model-only-skill","description":"Model-only skill."},
                {"name":"z-skill","description":"Long description Long description Long descript..."}
            ]
        })
        .as_object()
        .unwrap()
        .clone()
    );
    let text = catalog_message.content[0].text_value().unwrap();
    assert_eq!(
        text,
        [
            "<system-reminder>",
            "A skill is a reusable set of task-specific instructions. The following skills are available in this session:",
            "",
            "<available_skills>",
            "- `a-skill`: Use {{placeholder}} &lt;safely&gt; &amp; carefully.",
            "- `model-only-skill`: Model-only skill.",
            "- `z-skill`: Long description Long description Long descript...",
            "</available_skills>",
            "",
            "If the user names a skill, or the task clearly matches a skill's description, call the `skill` tool with the exact skill name before taking task actions. Load all applicable skills, then follow their full instructions. This catalog contains summaries only; do not infer or follow a skill's instructions until it has been loaded.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    let rendered = serde_json::to_string(catalog_message).unwrap();
    for hidden in [
        "whenToUse",
        "secret-source",
        "/secret/path",
        "Secret body.",
        "user-only-skill",
    ] {
        assert!(!rendered.contains(hidden), "leaked {hidden}");
    }
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(catalog_message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap())).is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_replacement_catalogs_for_additions_and_removals() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "first-skill",
        "First",
        "First body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let initial = enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())
        .pop()
        .unwrap();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(&initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    register_skill(
        &ctx,
        "second-skill",
        "Second",
        "Second body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let update = enter(pre_step(&ctx, agent, vec![]).await.unwrap())
        .pop()
        .unwrap();
    assert_eq!(update.source.data["update"], true);
    assert_eq!(update.source.data["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        update.content[0].text_value().unwrap(),
        [
            "<system-reminder>",
            "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:",
            "",
            "<available_skills>",
            "- `first-skill`: First",
            "- `second-skill`: Second",
            "</available_skills>",
            "",
            "Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the `skill` tool with the exact name before acting.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_an_empty_complete_tombstone_after_the_last_skill_is_removed() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let unregister = ctx
        .get::<Skills>()
        .unwrap()
        .register(
            &ctx,
            SkillRegistration::from_value(json!({
                "name":"temporary-skill","description":"Temporary","content":"Body.",
                "source":"runtime","provider":"runtime",
                "invocation":{"modelInvocable":true,"userInvocable":true}
            })),
        )
        .unwrap()
        .unwrap();
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    unregister.dispose().await;
    let tombstone = catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(tombstone.source.data["update"], true);
    assert_eq!(tombstone.source.data["entries"], json!([]));
    assert!(tombstone.source.data.get("incomplete").is_none());
    assert_eq!(
        tombstone.content[0].text_value().unwrap(),
        [
            "<system-reminder>",
            "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:",
            "",
            "<available_skills>",
            "</available_skills>",
            "",
            "No skills are currently available through the `skill` tool. Do not use names from earlier skill catalogs.",
            "A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool for it.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn ignores_malformed_durable_catalogs_and_republishes_after_compaction_hides_a_valid_one() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "durable-skill",
        "Durable",
        "Body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    for raw in [
        json!({"form":"catalog"}),
        json!({"form":"catalog","entries":null}),
        json!({"form":"catalog","entries":"not-an-array"}),
        json!({"form":"catalog","entries":[null]}),
        json!({"form":"catalog","entries":[{"name":"x"}]}),
        json!({"form":"catalog","entries":[{"description":"no name"}]}),
    ] {
        let mut source = MessageSource::new(SkillCatalogSource::KIND);
        source.data = raw.as_object().unwrap().clone();
        let malformed = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("unreadable catalog")],
            source,
        });
        agent
            .session()
            .append(
                "user/message",
                serde_json::to_value(malformed).unwrap(),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
    }
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert!(initial.source.data.get("update").is_none());
    let initial_event = agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let checkpoint = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("compacted history")],
        source: MessageSource::new("compact-checkpoint"),
    });
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(checkpoint).unwrap(),
            Some(SurfaceIntent::replace(
                initial_event.seq(),
                initial_event.seq(),
                vec![initial_event.seq()],
            )),
        )
        .unwrap();
    let restored = catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(restored.source.data["update"], true);
    assert_eq!(restored.source.data["entries"][0]["name"], "durable-skill");
    kernel.shutdown().await;
}

#[tokio::test]
async fn resumes_from_the_durable_entries_of_the_latest_visible_catalog() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "resumed-skill",
        "Resumed skill",
        "Resumed body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let mut old_source = MessageSource::new(SkillCatalogSource::KIND);
    old_source.data = json!({
        "form":"catalog",
        "entries":[{"name":"old-skill","description":"Old skill"}]
    })
    .as_object()
    .unwrap()
    .clone();
    let old = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("prose a reader cannot rely on")],
        source: old_source,
    });
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(old).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let foreign = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("- `resumed-skill`: Resumed skill")],
        source: MessageSource::new("dsh-tool-skill"),
    });
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(foreign).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();

    let update = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(
        update.source.data,
        json!({
            "form":"catalog","update":true,
            "entries":[{"name":"resumed-skill","description":"Resumed skill"}]
        })
        .as_object()
        .unwrap()
        .clone()
    );
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(update).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap())).is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn omits_confirmed_empty_catalog_but_publishes_incomplete_and_recovery() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "user-only",
        "Not model visible",
        "User body.",
        json!({"modelInvocable":false,"userInvocable":true}),
    );
    assert!(catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())).is_none());

    let provider = Arc::new(ProbeProvider::empty_failing());
    let control = register_probe(&ctx, provider.clone());
    let controller = AbortController::new();
    let decision = ctx
        .waterfall(
            AgentPreStep {
                agent: agent.clone(),
                messages: vec![],
                turn: 1,
                step: 1,
                signal: controller.signal(),
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            |_| async {
                Ok(PreStepDecision::Enter {
                    messages: vec![],
                    starts_request_series: false,
                })
            },
        )
        .await
        .unwrap();
    let incomplete = catalog(&enter(decision)).unwrap().clone();
    assert_eq!(incomplete.source.data["entries"], json!([]));
    assert_eq!(incomplete.source.data["incomplete"], true);
    assert_eq!(
        incomplete.content[0].text_value().unwrap(),
        [
            "<system-reminder>",
            "No skills could be confirmed in this discovery attempt.",
            "",
            "<available_skills>",
            "</available_skills>",
            "",
            "Some skills could not be checked. Only confirmed skills are listed; a lower-priority definition of the same name may be selected when a preferred source is unavailable. Discovery will be retried on the next request.",
            "",
            "Wait for an updated catalog before calling the `skill` tool. Do not infer skill instructions from an empty catalog.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    controller.abort();
    assert!(
        provider
            .seen_signal
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .aborted()
    );

    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(incomplete).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    provider.failing.store(false, Ordering::SeqCst);
    control.invalidate();
    let recovery = catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(
        recovery.source.data,
        json!({"form":"catalog","update":true,"entries":[]})
            .as_object()
            .unwrap()
            .clone()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn distinguishes_incomplete_removal_from_a_confirmed_empty_recovery() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let unregister = ctx
        .get::<Skills>()
        .unwrap()
        .register(
            &ctx,
            SkillRegistration::from_value(json!({
                "name":"recovering-skill","description":"Recovering","content":"Body.",
                "source":"runtime","provider":"runtime",
                "invocation":{"modelInvocable":true,"userInvocable":true}
            })),
        )
        .unwrap()
        .unwrap();
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    unregister.dispose().await;
    let provider = Arc::new(ProbeProvider::empty_failing());
    let control = register_probe(&ctx, provider.clone());
    let incomplete = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(incomplete.source.data["update"], true);
    assert_eq!(incomplete.source.data["incomplete"], true);
    assert_eq!(incomplete.source.data["entries"], json!([]));
    assert_eq!(
        incomplete.content[0].text_value().unwrap(),
        [
            "<system-reminder>",
            "Skill discovery is incomplete. This catalog of currently available skills replaces every earlier available-skills list in this session:",
            "",
            "<available_skills>",
            "</available_skills>",
            "",
            "Some skills could not be checked. Only confirmed skills are listed; a lower-priority definition of the same name may be selected when a preferred source is unavailable. Discovery will be retried on the next request.",
            "",
            "No skills could be confirmed in this discovery attempt. Do not use names from earlier skill catalogs.",
            "A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool for it.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(incomplete).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    provider.failing.store(false, Ordering::SeqCst);
    control.invalidate();
    let recovered = catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(recovered.source.data["update"], true);
    assert_eq!(recovered.source.data["entries"], json!([]));
    assert!(recovered.source.data.get("incomplete").is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_readable_skills_loadable_and_publishes_additions_during_incomplete_discovery() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let dispose_stable = ctx
        .get::<Skills>()
        .unwrap()
        .register(
            &ctx,
            SkillRegistration::from_value(json!({
                "name":"stable-skill","description":"Stable skill","content":"Stable body.",
                "source":"runtime","provider":"runtime",
                "invocation":{"modelInvocable":true,"userInvocable":true}
            })),
        )
        .unwrap()
        .unwrap();
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();

    let provider = Arc::new(ProbeProvider::empty_failing());
    register_probe(&ctx, provider);
    let incomplete = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(
        incomplete.source.data,
        json!({
            "form":"catalog","update":true,"incomplete":true,
            "entries":[{"name":"stable-skill","description":"Stable skill"}]
        })
        .as_object()
        .unwrap()
        .clone()
    );
    assert_eq!(
        incomplete.content[0].text_value().unwrap(),
        [
            "<system-reminder>",
            "Skill discovery is incomplete. This catalog of currently available skills replaces every earlier available-skills list in this session:",
            "",
            "<available_skills>",
            "- `stable-skill`: Stable skill",
            "</available_skills>",
            "",
            "Some skills could not be checked. Only confirmed skills are listed; a lower-priority definition of the same name may be selected when a preferred source is unavailable. Discovery will be retried on the next request.",
            "",
            "Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the `skill` tool with the exact name before acting.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.",
            "</system-reminder>",
        ]
        .join("\n")
    );
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(incomplete).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let mut input = ToolExecutionInput::new(
        "partial-load".into(),
        "skill",
        json!({"name":"stable-skill"}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent.clone());
    let loaded = execute_tool_fixture(&ctx, input).await.unwrap();
    assert!(!loaded.is_error(), "{:?}", loaded.error());
    assert!(
        loaded.content()[0]
            .text_value()
            .unwrap()
            .contains("Stable body.")
    );

    register_skill(
        &ctx,
        "new-skill",
        "New skill",
        "New body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    dispose_stable.dispose().await;
    let changed = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(changed.source.data["incomplete"], true);
    assert_eq!(
        changed.source.data["entries"],
        json!([{"name":"new-skill","description":"New skill"}])
    );
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(changed).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap())).is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_an_incomplete_initial_catalog_and_clears_its_notice_after_recovery() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let provider = Arc::new(ProbeProvider {
        failing: AtomicBool::new(false),
        complete: AtomicBool::new(false),
        list_calls: AtomicUsize::new(0),
        get_calls: AtomicUsize::new(0),
        seen_signal: Mutex::new(None),
        seen_cwd: Mutex::new(None),
        candidate: None,
        loaded: Mutex::new(None),
    });
    register_probe(&ctx, provider.clone());
    register_skill(
        &ctx,
        "readable-skill",
        "Readable",
        "Body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert_eq!(initial.source.data["incomplete"], true);
    assert_eq!(
        initial.source.data["entries"],
        json!([{"name":"readable-skill","description":"Readable"}])
    );
    assert!(
        initial.content[0]
            .text_value()
            .unwrap()
            .contains("Some skills could not be checked")
    );
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())).is_none());

    provider.complete.store(true, Ordering::SeqCst);
    let recovered = catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap()))
        .unwrap()
        .clone();
    assert!(recovered.source.data.get("incomplete").is_none());
    assert!(
        !recovered.content[0]
            .text_value()
            .unwrap()
            .contains("Some skills could not be checked")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn deduplicates_matching_proposed_catalog_and_replaces_a_stale_one_in_place() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "current-skill",
        "Current",
        "Body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let matching = enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())
        .pop()
        .unwrap();
    let matching_id = matching.id.clone();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(&matching).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let duplicate = enter(
        pre_step(&ctx, agent.clone(), vec![matching.clone()])
            .await
            .unwrap(),
    );
    assert!(duplicate.is_empty());

    register_skill(
        &ctx,
        "second-skill",
        "Second",
        "Second body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let companion = user("keep this message");
    let companion_id = companion.id.clone();
    let replaced = enter(
        pre_step(&ctx, agent, vec![companion, matching])
            .await
            .unwrap(),
    );
    assert_eq!(replaced[0].id, companion_id);
    assert_ne!(replaced[1].id, matching_id);
    assert_eq!(
        replaced[1].source.data["entries"],
        json!([
            {"name":"current-skill","description":"Current"},
            {"name":"second-skill","description":"Second"}
        ])
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_a_proposed_catalog_that_already_matches_the_current_snapshot() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "first-skill",
        "First skill",
        "First body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let proposed = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    let proposed_value = serde_json::to_value(&proposed).unwrap();
    let decision = enter(pre_step(&ctx, agent, vec![proposed]).await.unwrap());
    assert_eq!(decision.len(), 1);
    assert_eq!(serde_json::to_value(&decision[0]).unwrap(), proposed_value);
    kernel.shutdown().await;
}

#[tokio::test]
async fn removes_a_stale_proposed_catalog_before_the_first_empty_baseline() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let mut malformed_source = MessageSource::new(SkillCatalogSource::KIND);
    malformed_source
        .data
        .insert("form".into(), Value::String("catalog".into()));
    let malformed = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("preserve unreadable claimed context")],
        source: malformed_source,
    });
    let malformed_value = serde_json::to_value(&malformed).unwrap();
    let mut stale_source = MessageSource::new(SkillCatalogSource::KIND);
    stale_source.data = json!({
        "form":"catalog",
        "entries":[{"name":"stale-skill","description":"Stale skill"}]
    })
    .as_object()
    .unwrap()
    .clone();
    let stale = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("- `stale-skill`: Stale skill")],
        source: stale_source,
    });
    let decision = enter(pre_step(&ctx, agent, vec![malformed, stale]).await.unwrap());
    assert_eq!(decision.len(), 1);
    assert_eq!(serde_json::to_value(&decision[0]).unwrap(), malformed_value);
    kernel.shutdown().await;
}

#[tokio::test]
async fn user_gesture_injects_loaded_instructions_after_the_catalog() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "named-skill",
        "Named",
        "Full instructions.",
        json!({"modelInvocable":false,"userInvocable":true}),
    );
    register_skill(
        &ctx,
        "catalog-skill",
        "Catalog",
        "Catalog instructions.",
        json!({"modelInvocable":true,"userInvocable":false}),
    );
    let messages = enter(
        pre_step(
            &ctx,
            agent,
            vec![user("Please use /named-skill /named-skill")],
        )
        .await
        .unwrap(),
    );
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].source.kind, "user");
    assert_eq!(messages[1].source.kind, SkillCatalogSource::KIND);
    assert_eq!(
        messages[1].source.data["entries"],
        json!([{"name":"catalog-skill","description":"Catalog"}])
    );
    assert!(
        !serde_json::to_string(&messages[1])
            .unwrap()
            .contains("named-skill")
    );
    assert_eq!(messages[2].source.kind, "skill-invocation");
    assert_eq!(messages[2].source.data["name"], "named-skill");
    assert!(
        messages[2].content[0]
            .text_value()
            .unwrap()
            .contains("Full instructions.")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn unknown_and_user_disabled_gestures_remain_plain_prose() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "disabled-skill",
        "Disabled",
        "Hidden.",
        json!({"modelInvocable":true,"userInvocable":false}),
    );
    let messages = enter(
        pre_step(
            &ctx,
            agent,
            vec![user("/unknown-skill /disabled-skill /usr/bin 5/8")],
        )
        .await
        .unwrap(),
    );
    assert!(
        messages
            .iter()
            .all(|message| message.source.kind != "skill-invocation")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn loads_a_skill_and_returns_provider_managed_resource_guidance() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "loadable-skill",
        "Loadable",
        "Loaded body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    let mut input = ToolExecutionInput::new(
        "call-skill".into(),
        "skill",
        json!({"name":"loadable-skill"}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    let result = execute_tool_fixture(&ctx, input).await.unwrap();
    assert!(!result.is_error(), "{:?}", result.error());
    assert_eq!(
        result.value(),
        Some(&json!({"name":"loadable-skill","provider":"runtime","content":"Loaded body."}))
    );
    let text = result.content()[0].text_value().unwrap();
    assert!(text.contains("Resources for this skill are managed by provider \"runtime\"."));
    assert!(text.contains("Loaded body."));
    kernel.shutdown().await;
}

#[tokio::test]
async fn loads_a_filesystem_skill_with_exact_content_and_directory_resource_base() {
    let (kernel, ctx, home, agent) = setup(json!({})).await;
    let skill_directory = home.path().join(".dsh/skills/project-skill");
    std::fs::create_dir_all(&skill_directory).unwrap();
    std::fs::write(
        skill_directory.join("SKILL.md"),
        "---\nname: project-skill\ndescription: Project skill\n---\n\nProject instructions.\n",
    )
    .unwrap();
    ctx.plugin::<SkillFileSystemPlugin>(json!({
        "dshHome":home.path().join(".dsh"),
        "agentsHome":home.path().join(".agents"),
        "watch":false
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    let mut input = ToolExecutionInput::new(
        "call-project-skill".into(),
        "skill",
        json!({"name":"project-skill"}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    let result = execute_tool_fixture(&ctx, input).await.unwrap();
    assert!(!result.is_error(), "{:?}", result.error());
    let directory = skill_directory.to_str().unwrap();
    assert_eq!(
        result.value(),
        Some(&json!({
            "name":"project-skill",
            "provider":"filesystem",
            "resourceBase":{"kind":"directory","path":directory},
            "content":"Project instructions."
        }))
    );
    assert_eq!(
        result.content()[0].text_value().unwrap(),
        [
            "<skill_content name=\"project-skill\">",
            "<skill_resources>",
            &format!("Base directory for this skill: {directory}"),
            "Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.",
            "</skill_resources>",
            "",
            "<skill_instructions>",
            "Project instructions.",
            "</skill_instructions>",
            "</skill_content>",
        ]
        .join("\n")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn unknown_invalid_and_model_disabled_calls_are_errors() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    register_skill(
        &ctx,
        "disabled-skill",
        "Disabled",
        "Hidden.",
        json!({"modelInvocable":false,"userInvocable":true}),
    );
    for (name, expected_message, expected_name, expected_code) in [
        (
            "missing",
            "skill \"missing\" is unknown or no longer available",
            "SkillError",
            "SKILL_NOT_FOUND",
        ),
        (
            "Bad_Name",
            "invalid arguments: invalid skill name \"Bad_Name\"",
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            "disabled-skill",
            "skill \"disabled-skill\" is not available for model invocation",
            "SkillError",
            "SKILL_NOT_MODEL_INVOCABLE",
        ),
    ] {
        let mut input = ToolExecutionInput::new(
            format!("call-{name}").into(),
            "skill",
            json!({"name":name}),
            AbortController::new().signal(),
        );
        input.agent = Some(agent.clone());
        let result = execute_tool_fixture(&ctx, input).await.unwrap();
        assert!(result.is_error());
        let failure = result.error().unwrap();
        assert_eq!(failure.message, expected_message);
        assert_eq!(
            failure.info.as_ref().map(|info| (&*info.name, &*info.code)),
            Some((expected_name, expected_code))
        );
        let text = result.content()[0].text_value().unwrap();
        assert!(text.contains(expected_message));
        assert!(!text.contains("Hidden."));
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn checks_model_policy_before_loading_and_rechecks_the_loaded_definition() {
    for (summary_policy, loaded_policy, loaded_exists, expected_gets, expected_code) in [
        (false, true, true, 0, "SKILL_NOT_MODEL_INVOCABLE"),
        (true, false, true, 1, "SKILL_NOT_MODEL_INVOCABLE"),
        (true, true, false, 1, "SKILL_NOT_FOUND"),
    ] {
        let (kernel, ctx, _home, agent) = setup(json!({})).await;
        let name = if summary_policy {
            "loaded-disabled"
        } else {
            "summary-disabled"
        };
        let provider = Arc::new(ProbeProvider {
            failing: AtomicBool::new(false),
            complete: AtomicBool::new(true),
            list_calls: AtomicUsize::new(0),
            get_calls: AtomicUsize::new(0),
            seen_signal: Mutex::new(None),
            seen_cwd: Mutex::new(None),
            candidate: Some(SkillCandidate::from_value(json!({
                "name":name,
                "description":"Policy probe",
                "invocation":{"modelInvocable":summary_policy,"userInvocable":true},
                "provider":"probe",
                "source":"probe",
                "rank":10,
                "locator":{"id":name}
            }))),
            loaded: Mutex::new(loaded_exists.then(|| {
                SkillDefinition::from_value(json!({
                    "name":name,
                    "description":"Policy probe",
                    "invocation":{"modelInvocable":loaded_policy,"userInvocable":true},
                    "provider":"probe",
                    "source":"probe",
                    "content":"Must not be returned."
                }))
            })),
        });
        register_probe(&ctx, provider.clone());
        let mut input = ToolExecutionInput::new(
            format!("call-{name}").into(),
            "skill",
            json!({"name":name}),
            AbortController::new().signal(),
        );
        input.agent = Some(agent);
        let result = execute_tool_fixture(&ctx, input).await.unwrap();
        assert!(result.is_error());
        assert_eq!(provider.get_calls.load(Ordering::SeqCst), expected_gets);
        let failure = result.error().unwrap();
        assert_eq!(
            failure.info.as_ref().map(|info| (&*info.name, &*info.code)),
            Some(("SkillError", expected_code))
        );
        let expected_message = if expected_code == "SKILL_NOT_FOUND" {
            format!("skill \"{name}\" is unknown or no longer available")
        } else {
            format!("skill \"{name}\" is not available for model invocation")
        };
        assert_eq!(failure.message, expected_message);
        let text = result.content()[0].text_value().unwrap();
        assert!(text.contains(&expected_message));
        assert!(!text.contains("Must not be returned."));
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn body_only_edits_do_not_change_the_catalog_but_load_the_latest_body() {
    let (kernel, ctx, home, agent) = setup(json!({})).await;
    let provider = Arc::new(ProbeProvider {
        failing: AtomicBool::new(false),
        complete: AtomicBool::new(true),
        list_calls: AtomicUsize::new(0),
        get_calls: AtomicUsize::new(0),
        seen_signal: Mutex::new(None),
        seen_cwd: Mutex::new(None),
        candidate: Some(SkillCandidate::from_value(json!({
            "name":"editable-skill",
            "description":"Stable description",
            "invocation":{"modelInvocable":true,"userInvocable":true},
            "provider":"probe","source":"probe","rank":10,"locator":{"id":"editable"}
        }))),
        loaded: Mutex::new(Some(SkillDefinition::from_value(json!({
            "name":"editable-skill",
            "description":"Stable description",
            "invocation":{"modelInvocable":true,"userInvocable":true},
            "provider":"probe","source":"probe","content":"Body one."
        })))),
    });
    register_probe(&ctx, provider.clone());
    let initial = catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
        .unwrap()
        .clone();
    agent
        .session()
        .append(
            "user/message",
            serde_json::to_value(initial).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    *provider.loaded.lock().unwrap() = Some(SkillDefinition::from_value(json!({
        "name":"editable-skill",
        "description":"Stable description",
        "invocation":{"modelInvocable":true,"userInvocable":true},
        "provider":"probe","source":"probe","content":"Body two."
    })));
    assert!(catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())).is_none());

    let mut input = ToolExecutionInput::new(
        "call-editable".into(),
        "skill",
        json!({"name":"editable-skill"}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    let result = execute_tool_fixture(&ctx, input).await.unwrap();
    assert_eq!(result.value().unwrap()["content"], "Body two.");
    assert_eq!(
        provider
            .seen_cwd
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|cwd| cwd.as_deref()),
        Some(home.path().to_str().unwrap())
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn resolves_scoped_skills_and_hides_catalog_when_the_tool_is_restricted() {
    let (kernel, ctx, _home, _global_agent) = setup(json!({})).await;
    let key = ScopeKey::new();
    let scope = create_scope(&ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let agent = Arc::new(StubAgent {
        id: SessionId::from("scoped-tool-skill"),
        session: Session::create(SessionId::from("scoped-tool-skill"), None, None, None, None)
            .unwrap(),
        ctx: scope.ctx.clone(),
        scope: key,
        options: AgentOptions::default(),
        model: unsupported_model(),
        inbox: unsupported_inbox(),
    }) as Arc<dyn Agent>;
    register_skill(
        &scope.ctx,
        "scoped-skill",
        "Only this agent",
        "Scoped body.",
        json!({"modelInvocable":true,"userInvocable":true}),
    );
    assert_eq!(
        catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap()))
            .unwrap()
            .source
            .data["entries"][0]["name"],
        "scoped-skill"
    );
    let shadow = define_tool(DefineToolOptions::new(
        "skill",
        "scoped shadow",
        json!({}),
        DefineToolOutput {
            schema: json!({"type":"string"}),
            render: Arc::new(|_, value| vec![ContentBlock::text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("shadow")) }),
    ))
    .unwrap();
    let unshadow = ctx
        .get::<Tools>()
        .unwrap()
        .register(&scope.ctx, shadow, ToolRegistrationOptions::default())
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent.clone(), vec![]).await.unwrap())).is_none());
    unshadow.dispose().await;
    ctx.get::<Tools>()
        .unwrap()
        .restrict(
            &scope.ctx,
            hoh_tools::ToolRestriction {
                allow: None,
                deny: Some(vec!["skill".into()]),
            },
        )
        .unwrap();
    assert!(catalog(&enter(pre_step(&ctx, agent, vec![]).await.unwrap())).is_none());
    scope.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_all_three_resource_base_variants_at_the_tool_boundary() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    for (name, resource_base) in [
        (
            "directory-skill",
            json!({"kind":"directory","path":"/skills/directory"}),
        ),
        (
            "url-skill",
            json!({"kind":"url","url":"https://skills.invalid/base/"}),
        ),
        (
            "opaque-skill",
            json!({"kind":"opaque","description":"Ask the provider"}),
        ),
    ] {
        ctx.get::<Skills>()
            .unwrap()
            .register(
                &ctx,
                SkillRegistration::from_value(json!({
                    "name":name,
                    "description":"Resource skill",
                    "content":"Body.",
                    "source":"runtime",
                    "provider":"runtime",
                    "resourceBase":resource_base,
                    "invocation":{"modelInvocable":true,"userInvocable":true}
                })),
            )
            .unwrap();
        let mut input = ToolExecutionInput::new(
            format!("call-{name}").into(),
            "skill",
            json!({"name":name}),
            AbortController::new().signal(),
        );
        input.agent = Some(agent.clone());
        let result = execute_tool_fixture(&ctx, input).await.unwrap();
        assert!(!result.is_error(), "{:?}", result.error());
        assert_eq!(result.value().unwrap()["resourceBase"], resource_base);
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_unknown_resource_base_kind_at_the_canonical_boundary() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let provider = Arc::new(ProbeProvider {
        failing: AtomicBool::new(false),
        complete: AtomicBool::new(true),
        list_calls: AtomicUsize::new(0),
        get_calls: AtomicUsize::new(0),
        seen_signal: Mutex::new(None),
        seen_cwd: Mutex::new(None),
        candidate: Some(SkillCandidate::from_value(json!({
            "name":"invalid-resource","description":"Invalid resource",
            "invocation":{"modelInvocable":true,"userInvocable":true},
            "provider":"probe","source":"probe","rank":10,"locator":{"id":"invalid"}
        }))),
        loaded: Mutex::new(Some(SkillDefinition::from_value(json!({
            "name":"invalid-resource","description":"Invalid resource",
            "invocation":{"modelInvocable":true,"userInvocable":true},
            "provider":"probe","source":"probe","content":"Body.",
            "resourceBase":{"kind":"bucket","name":"skills"}
        })))),
    });
    register_probe(&ctx, provider);
    let mut input = ToolExecutionInput::new(
        "call-invalid-resource".into(),
        "skill",
        json!({"name":"invalid-resource"}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    let result = execute_tool_fixture(&ctx, input).await.unwrap();
    assert!(result.is_error());
    let failure = result.error().unwrap();
    assert_eq!(failure.info.as_ref().unwrap().code, "INVALID_TOOL_OUTPUT");
    assert!(
        result.content()[0]
            .text_value()
            .unwrap()
            .contains("value.resourceBase")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn validates_catalog_description_cap() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let home = TempDir::new().unwrap();
    provide_working_directory_fixture(&ctx, Some(home.path().to_str().unwrap()))
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    for value in [json!(2), json!(3.5)] {
        let error = ctx
            .plugin::<ToolSkillPlugin>(json!({"catalogDescriptionMaxLength":value}))
            .unwrap()
            .settled()
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("integer greater than or equal to 3")
        );
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn accepts_unknown_configuration_fields_like_the_source_object_schema() {
    let (kernel, _ctx, _home, _agent) = setup(json!({
        "catalogDescriptionMaxLength": 50,
        "extensionField": true
    }))
    .await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn both_listeners_pass_downstream_reject_through_untouched() {
    let (kernel, ctx, _home, agent) = setup(json!({})).await;
    let decision = ctx
        .waterfall(
            AgentPreStep {
                agent,
                messages: vec![user("/anything")],
                turn: 1,
                step: 1,
                signal: AbortController::new().signal(),
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            |_| async { Ok(PreStepDecision::Reject) },
        )
        .await
        .unwrap();
    assert_eq!(decision, PreStepDecision::Reject);
    kernel.shutdown().await;
}

#[test]
fn config_defaults_match_the_source() {
    assert_eq!(Config::default().catalog_description_max_length, 500.0);
}
