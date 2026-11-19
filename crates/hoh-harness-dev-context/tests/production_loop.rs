//! Production-loop regression for the source package's cross-process stability contract.

use hoh_agent::{
    Agent, AgentHandle, AgentOptions, AgentPreStep, CreateAgentOptions, Live, PreStepDecision,
};
use hoh_agent_loop_testkit::{
    AgentLoopTestHarness, mount_agent_loop_test_dependencies, mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_harness_dev_context::{HarnessDevContext, WebApp, WebAppApi};
use hoh_kernel::{Context, Kernel, Next};
use hoh_llm::{
    AdapterRegistrationHandle, ContentBlock, GenerateOptions, Llm, LlmAdapter, LlmFuture,
    LlmModelInfo, LlmResolvedModelInfo, LlmStream, MessageSource, NewUserMessage, StreamChunk,
    create_user_message,
};
use hoh_session::{Session, SessionId};
use hoh_system_prompt::{PromptPresentation, PromptSection, SystemPromptAssemble, SystemPromptKey};
use serde_json::json;
use std::sync::{Arc, Mutex, MutexGuard};

const LEGACY_SOURCE: &str = "The DeepSeek Harness implementation checkout is at /old/checkout/. The checkout location and current working directory are separate values and may differ; never infer the working directory from this path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";
const LEGACY_WEB: &str = "You are interacting with the user through the DeepSeek Harness Web GUI at http://127.0.0.1:3100. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";
const SOURCE_PROMPT: &str = "The runtime context names the DeepSeek Harness implementation checkout. The checkout location and current working directory are separate values and may differ; never infer the working directory from the checkout path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";
const WEB_PROMPT: &str = "You are interacting with the user through the DeepSeek Harness Web GUI whose URL the runtime context names. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct MutableWebApp(Mutex<String>);

impl WebAppApi for MutableWebApp {
    fn get_advertised_url(&self) -> String {
        lock(&self.0).clone()
    }
}

impl MutableWebApp {
    fn set_port(&self, port: u16) {
        *lock(&self.0) = format!("http://127.0.0.1:{port}");
    }
}

fn chunk(value: serde_json::Value) -> StreamChunk {
    serde_json::from_value(value).unwrap()
}

fn response() -> Vec<StreamChunk> {
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"text-delta","index":0,"text":"ok"})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":"ok"}})),
        chunk(json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":1}})),
        chunk(json!({"type":"finish","reason":{"kind":"stop"}})),
    ]
}

struct RecordingAdapter {
    update: Option<String>,
    requests: Mutex<Vec<GenerateOptions>>,
}

impl LlmAdapter for RecordingAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let update = self.update.clone();
        Box::pin(async move {
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                system_prompt_update: update,
                ..Default::default()
            })
        })
    }

    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        lock(&self.requests).push(options);
        Ok(LlmStream::chunks(response()))
    }
}

fn route() -> AgentOptions {
    AgentOptions {
        provider: Some(Live::Value("route".into())),
        model: Some(Live::Value("model".into())),
        ..AgentOptions::default()
    }
}

fn user_message(text: &str) -> hoh_llm::UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

async fn send(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).unwrap();
    agent.when_idle().await;
}

fn system_prompts(session: &Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "system/message")
        .map(|event| {
            event.data()["message"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|block| {
                    (block["type"] == "text")
                        .then(|| block["text"].as_str())
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

fn latest_runtime_context(session: &Session) -> Option<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|event| {
            event.event_type() == "user/message"
                && event.data()["source"]["kind"] == "runtime-context"
        })
        .and_then(|event| event.data()["content"].as_array())
        .and_then(|content| content.first())
        .and_then(|block| block["text"].as_str())
        .map(str::to_owned)
}

fn request_system_prompts(request: &GenerateOptions) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role() == "system")
        .map(|message| {
            message
                .content()
                .iter()
                .filter_map(ContentBlock::text_value)
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

struct LoopFixture {
    kernel: Kernel,
    ctx: Context,
    web: Arc<MutableWebApp>,
    adapter: Arc<RecordingAdapter>,
    loop_harness: AgentLoopTestHarness,
    _registration: AdapterRegistrationHandle,
}

impl LoopFixture {
    async fn new(update: Option<&str>) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(&ctx, Default::default())
            .await
            .unwrap();
        let web = Arc::new(MutableWebApp(Mutex::new("http://127.0.0.1:4567".into())));
        ctx.provide::<WebApp>(web.clone()).await.unwrap();
        ctx.plugin::<HarnessDevContext>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let loop_harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        let adapter = Arc::new(RecordingAdapter {
            update: update.map(str::to_owned),
            requests: Mutex::new(Vec::new()),
        });
        let registration = ctx
            .get::<Llm>()
            .unwrap()
            .register_adapter(&ctx, vec!["route".into()], adapter.clone())
            .unwrap();
        Self {
            kernel,
            ctx,
            web,
            adapter,
            loop_harness,
            _registration: registration,
        }
    }

    async fn earlier_session(&self, source: &str, web: &str, id: &str) -> Arc<dyn Agent> {
        let source = source.to_owned();
        let web = web.to_owned();
        let listener = self
            .ctx
            .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(
                move |_event, next: Next<PromptPresentation>| {
                    let source = source.clone();
                    let web = web.clone();
                    async move {
                        let presentation = next.run().await?;
                        presentation.update(|assembly| {
                            for section in &mut assembly.sections {
                                if section.name == "harness:source" {
                                    section.text.clone_from(&source);
                                } else if section.name == "app:web-surface" {
                                    section.text.clone_from(&web);
                                }
                            }
                            assembly.contexts.retain(|entry| {
                                entry.name != "harness:source" && entry.name != "app:web-surface"
                            });
                        });
                        Ok(presentation)
                    }
                },
            )
            .unwrap();
        let agent = self
            .loop_harness
            .create_with(id.into(), route(), Default::default())
            .await
            .unwrap();
        send(&agent, "before the upgrade").await;
        listener.dispose().await;
        agent
    }

    async fn resume(&self, source: &Session, id: &str) -> AgentHandle {
        let mut options = CreateAgentOptions::new(SessionId::from(id));
        options.seed = Some(source.snapshot_events(None, None).to_vec());
        options.agent_options = Some(route());
        self.ctx
            .get::<hoh_agent::Agents>()
            .unwrap()
            .create(&self.ctx, options)
            .await
            .unwrap()
    }

    fn latest_request_prompts(&self) -> Vec<String> {
        let request = lock(&self.adapter.requests).last().unwrap().clone();
        request_system_prompts(&request)
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

async fn legacy_round_trip(update: Option<&str>, label: &str) {
    let fixture = LoopFixture::new(update).await;
    let original = fixture
        .earlier_session(LEGACY_SOURCE, LEGACY_WEB, "earlier")
        .await;

    let logged = system_prompts(original.session());
    assert_eq!(logged.len(), 1);
    assert!(logged[0].contains(LEGACY_SOURCE));
    assert!(logged[0].contains(LEGACY_WEB));

    fixture.web.set_port(4568);
    let resumed = fixture
        .resume(original.session(), &format!("{label}-4568"))
        .await;
    send(&resumed.agent, "after the upgrade").await;
    assert_eq!(system_prompts(resumed.agent.session()), logged);
    assert_eq!(fixture.latest_request_prompts(), logged);
    let runtime = latest_runtime_context(resumed.agent.session()).unwrap();
    assert!(runtime.contains("Current DeepSeek Harness implementation checkout: \""));
    assert!(runtime.contains("Current DeepSeek Harness Web GUI URL: \"http://127.0.0.1:4568\"."));
    resumed.dispose().await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn production_loop_preserves_legacy_prompt_for_replace_only_and_in_history_routes() {
    legacy_round_trip(None, "replace-only").await;
    legacy_round_trip(Some("in-history"), "in-history").await;
}

#[tokio::test]
async fn production_loop_keeps_legacy_paragraphs_after_series_consolidation() {
    let fixture = LoopFixture::new(Some("in-history")).await;
    let original = fixture
        .earlier_session(LEGACY_SOURCE, LEGACY_WEB, "consolidated-source")
        .await;
    let earlier = system_prompts(original.session());
    assert_eq!(earlier.len(), 1);
    let resumed = fixture
        .resume(original.session(), "consolidated-resumed")
        .await;

    let update = fixture
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .section(
            &fixture.ctx,
            PromptSection {
                name: "test:update".into(),
                order: 10_050.0,
                text: "Updated instruction.".into(),
                ..PromptSection::default()
            },
        )
        .unwrap();
    send(&resumed.agent, "append update").await;
    let updated = system_prompts(resumed.agent.session());
    assert_eq!(updated.len(), 2);
    assert_eq!(updated[0], earlier[0]);
    assert!(updated[1].contains("Updated instruction."));
    update.dispose().await;

    let new_series = fixture
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|_event, next| async move {
            Ok(match next.run().await? {
                PreStepDecision::Reject => PreStepDecision::Reject,
                PreStepDecision::Enter { messages, .. } => PreStepDecision::Enter {
                    messages,
                    starts_request_series: true,
                },
            })
        })
        .unwrap();
    send(&resumed.agent, "start a new request series").await;
    let consolidated = system_prompts(resumed.agent.session());
    assert_eq!(consolidated.len(), 3);
    assert_eq!(consolidated[0], earlier[0]);
    assert!(consolidated[1].contains("Updated instruction."));
    assert!(consolidated[2].is_empty());
    assert_eq!(fixture.latest_request_prompts(), earlier);

    send(&resumed.agent, "continue the request series").await;
    assert_eq!(system_prompts(resumed.agent.session()), consolidated);
    assert_eq!(fixture.latest_request_prompts(), earlier);
    new_series.dispose().await;
    resumed.dispose().await.unwrap();
    fixture.close().await;
}

async fn production_resemblance_round_trip(source: &str, id: &str) {
    let fixture = LoopFixture::new(None).await;
    let original = fixture
        .earlier_session(source, WEB_PROMPT, "resembling")
        .await;
    let resumed = fixture.resume(original.session(), id).await;
    send(&resumed.agent, "after the upgrade").await;

    let logged = system_prompts(resumed.agent.session());
    assert_eq!(logged.len(), 2);
    assert!(logged[1].contains(SOURCE_PROMPT));
    assert!(logged[1].contains(WEB_PROMPT));
    assert!(!logged[1].contains("/old/"));
    assert_eq!(fixture.latest_request_prompts(), vec![logged[1].clone()]);
    resumed.dispose().await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn production_loop_replaces_resembling_legacy_source_paragraphs() {
    production_resemblance_round_trip(
        "The DeepSeek Harness implementation checkout is at /old/checkout/.",
        "resembling-ending",
    )
    .await;
    production_resemblance_round_trip(
        &LEGACY_SOURCE.replace("/old/checkout/", "/old/\ncheckout/"),
        "resembling-newline",
    )
    .await;
}

#[tokio::test]
async fn production_loop_keeps_fresh_prompt_value_free_across_ports() {
    let fixture = LoopFixture::new(None).await;
    let original = fixture
        .loop_harness
        .create_with("fresh".into(), route(), Default::default())
        .await
        .unwrap();
    send(&original, "first turn").await;
    let logged = system_prompts(original.session());
    assert_eq!(logged.len(), 1);
    assert!(logged[0].contains(SOURCE_PROMPT));
    assert!(logged[0].contains(WEB_PROMPT));
    assert!(!logged[0].contains("127.0.0.1"));
    let initial_runtime = latest_runtime_context(original.session()).unwrap();
    assert!(initial_runtime.contains("http://127.0.0.1:4567"));

    fixture.web.set_port(4568);
    let resumed = fixture.resume(original.session(), "fresh-on-4568").await;
    send(&resumed.agent, "second turn").await;
    assert_eq!(system_prompts(resumed.agent.session()), logged);
    assert_eq!(fixture.latest_request_prompts(), logged);
    let resumed_runtime = latest_runtime_context(resumed.agent.session()).unwrap();
    assert!(resumed_runtime.contains("http://127.0.0.1:4568"));
    resumed.dispose().await.unwrap();
    fixture.close().await;
}
