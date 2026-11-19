use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, Live, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_attachment::{
    AttachmentId, AttachmentStore, Attachments, ImageAttachmentLimits, ImageAttachmentRef,
    ImageMediaType, SaveImageAttachment, StoredImageAttachment,
};
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_fs_local::{Config as FsConfig, LocalFileSystem};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{
    ContentBlock, GenerateOptions, ImageBlock, Llm, LlmAdapter, LlmImageRequestPrice,
    LlmImageRequestPricing, LlmPlugin, LlmStream, MessageId, MessageSource, NewUserMessage,
    UserMessage, create_user_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_spill::{SaveTextSpill, SpillRef, SpillStore, SpillStoreApi, spill_locator};
use hoh_spill_policy::{SpillPolicy, shim::PtcDispatchLog};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tool_testkit::{
    ContentToolFixtureOptions, define_content_tool_fixture, execute_tool_fixture,
};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, ToolExecution, ToolExecutionFailure,
    ToolExecutionInput, ToolExecutionResult, ToolFailure, ToolRegistrationOptions, ToolRuntime,
    Tools, ToolsExecute, ToolsPostExecute, define_tool,
};
use serde_json::json;

struct TestAttachments {
    limits: ImageAttachmentLimits,
    path: String,
}

impl TestAttachments {
    fn new(path: String) -> Self {
        Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 1_024,
                max_images_per_message: 10,
                max_message_image_bytes: 10_240,
                max_image_pixels: 1_024,
                max_image_dimension: 1_024,
                media_types: vec![ImageMediaType::Png],
            },
            path,
        }
    }
}

impl AttachmentStore for TestAttachments {
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
        _: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async { Err("unused image save".into()) })
    }

    fn read_image<'a>(
        &'a self,
        _: &'a ImageAttachmentRef,
        _: Option<&'a hoh_timeout::AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err("unused image read".into()) })
    }

    fn image_host_path(
        &self,
        _: &ImageAttachmentRef,
    ) -> Result<Option<String>, hoh_error::HarnessError> {
        Ok(Some(self.path.clone()))
    }
}

struct TestPricing {
    inconsistent: bool,
}

impl LlmImageRequestPricing for TestPricing {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        if self.inconsistent {
            return Vec::new();
        }
        images
            .iter()
            .map(|_| LlmImageRequestPrice {
                visual_tokens: 10,
                text: String::new(),
            })
            .collect()
    }
}

struct VisionAdapter {
    pricing: Option<Arc<dyn LlmImageRequestPricing>>,
}

impl LlmAdapter for VisionAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        Ok(LlmStream::chunks([]))
    }

    fn image_request_pricing(
        &self,
        provider: &str,
        model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        (provider == "vision" && model == "vision")
            .then(|| self.pricing.clone())
            .flatten()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct StubStore {
    saves: Mutex<Vec<SaveTextSpill>>,
    fail: AtomicBool,
}

impl SpillStoreApi for StubStore {
    fn save_text(
        &self,
        input: SaveTextSpill,
    ) -> Pin<Box<dyn Future<Output = Result<SpillRef, BoxError>> + Send + '_>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                return Err("disk full".into());
            }
            let reference = SpillRef {
                locator: spill_locator(format!("/spill/{}", input.suggested_name)),
                bytes: input.content.len() as u64,
                retrieval_hint: "Use the stub retrieval path.".into(),
            };
            lock(&self.saves).push(input);
            Ok(reference)
        })
    }
}

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("the test agent has no model".into())
    }
}

struct UnusedInbox;

impl UnusedInbox {
    fn refuse<T>() -> Result<T, BoxError> {
        Err("the test agent has no inbox".into())
    }
}

impl Inbox for UnusedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn next_step(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn clear(&self) -> Result<(), BoxError> {
        Self::refuse()
    }

    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::refuse()
    }

    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::refuse()
    }

    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Self::refuse()
    }

    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Self::refuse()
    }

    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Self::refuse()
    }
}

struct TestAgent {
    scope: ScopeKey,
    session: Session,
    ctx: Context,
    options: AgentOptions,
}

impl TestAgent {
    fn new(ctx: &Context, id: &str) -> Arc<Self> {
        Self::with_route(ctx, id, Some("vision"), Some("vision"))
    }

    fn with_route(
        ctx: &Context,
        id: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Arc<Self> {
        Arc::new(Self {
            scope: ScopeKey::new(),
            session: Session::create(id.into(), None, None, None, None).unwrap(),
            ctx: ctx.clone(),
            options: AgentOptions {
                provider: provider.map(|value| Live::Value(value.into())),
                model: model.map(|value| Live::Value(value.into())),
                ..Default::default()
            },
        })
    }
}

impl Agent for TestAgent {
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
        &UnusedModel
    }

    fn inbox(&self) -> &dyn Inbox {
        &UnusedInbox
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

struct Stack {
    kernel: Kernel,
    ctx: Context,
    store: Option<Arc<StubStore>>,
    policy: Fiber,
}

async fn setup(config: serde_json::Value, with_store: bool) -> Stack {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let store = with_store.then(|| Arc::new(StubStore::default()));
    if let Some(store) = &store {
        let store = store.clone();
        ctx.inject(move |scope, (): ()| {
            let store = store.clone();
            async move {
                scope.provide::<SpillStore>(store).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
    }
    let policy = ctx.plugin::<SpillPolicy>(config).unwrap();
    policy.settled().await.unwrap();
    Stack {
        kernel,
        ctx,
        store,
        policy,
    }
}

fn register(stack: &Stack, name: &str, content: Vec<ContentBlock>) {
    let fixture = define_content_tool_fixture(ContentToolFixtureOptions::new(
        name,
        name,
        json!({}),
        move |_, _| {
            let content = content.clone();
            async move { Ok(content) }
        },
    ))
    .unwrap();
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .register(&stack.ctx, fixture, ToolRegistrationOptions::default())
        .unwrap();
}

async fn execute(stack: &Stack, name: &str, owner: bool) -> hoh_tools::ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        format!("call-{name}").into(),
        name,
        json!({}),
        AbortController::new().signal(),
    );
    if owner {
        input.agent = Some(TestAgent::new(&stack.ctx, "s1"));
    }
    execute_tool_fixture(&stack.ctx, input).await.unwrap()
}

async fn execute_as(
    stack: &Stack,
    name: &str,
    agent: Arc<dyn Agent>,
) -> hoh_tools::ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        format!("call-{name}").into(),
        name,
        json!({}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    execute_tool_fixture(&stack.ctx, input).await.unwrap()
}

async fn seed_execution(stack: &Stack) -> Arc<ToolExecution> {
    let captured = Arc::new(Mutex::new(None::<Arc<ToolExecution>>));
    let target = captured.clone();
    let fixture = define_content_tool_fixture(ContentToolFixtureOptions::new(
        "seed",
        "seed",
        json!({}),
        move |_, exec| {
            *lock(&target) = Some(exec);
            async { Ok(vec![ContentBlock::text("tiny")]) }
        },
    ))
    .unwrap();
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .register(&stack.ctx, fixture, ToolRegistrationOptions::default())
        .unwrap();
    execute(stack, "seed", true).await;
    lock(&captured).clone().unwrap()
}

async fn dispatch_log(
    stack: &Stack,
    exec: Arc<ToolExecution>,
    name: &str,
    sub_call_id: &str,
    content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    stack
        .ctx
        .waterfall(
            PtcDispatchLog {
                exec,
                name: name.into(),
                sub_call_id: sub_call_id.into(),
            },
            move |_| async move { Ok(content) },
        )
        .await
        .unwrap()
}

fn warning_messages(stack: &Stack) -> Vec<String> {
    stack
        .ctx
        .logger()
        .buffer()
        .into_iter()
        .filter(|record| record.level == hoh_kernel::Level::Warn)
        .map(|record| record.message)
        .collect()
}

fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(ContentBlock::text_value)
        .collect()
}

fn image(ordinal: char) -> ContentBlock {
    ImageBlock {
        attachment: ImageAttachmentRef {
            attachment_id: AttachmentId::from(format!("sha256:{}", ordinal.to_string().repeat(64))),
            media_type: ImageMediaType::Png,
            bytes: 1,
            width: 1,
            height: 1,
            name: None,
            original_dimensions: None,
        },
        offloaded: None,
    }
    .into()
}

async fn mount_visual_services(
    stack: &Stack,
    path: String,
    pricing: Option<Arc<dyn LlmImageRequestPricing>>,
    attachments: bool,
    fs: bool,
) {
    stack
        .ctx
        .plugin::<LlmPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    stack
        .ctx
        .get::<Llm>()
        .unwrap()
        .register_adapter(
            &stack.ctx,
            vec!["vision".into()],
            Arc::new(VisionAdapter { pricing }),
        )
        .unwrap();
    if attachments {
        let store = Arc::new(TestAttachments::new(path.clone()));
        stack
            .ctx
            .inject(move |scope, (): ()| {
                let store = store.clone();
                async move {
                    scope.provide::<Attachments>(store).await?;
                    Ok(())
                }
            })
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    if fs {
        let local = Arc::new(
            LocalFileSystem::new(FsConfig {
                cwd: Some(path),
                ..Default::default()
            })
            .unwrap(),
        );
        stack
            .ctx
            .inject(move |scope, (): ()| {
                let local = local.clone();
                async move {
                    scope.provide::<Fs>(local).await?;
                    Ok(())
                }
            })
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn omitted_cap_is_a_true_noop() {
    let stack = setup(json!({}), true).await;
    register(&stack, "big", vec![ContentBlock::text("x".repeat(1_000))]);
    let result = execute(&stack, "big", true).await;
    assert_eq!(text_of(result.content()), "x".repeat(1_000));
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_invalid_caps_at_load() {
    for (cap, expected) in [
        (
            json!(-1),
            "spill-policy: maxInlineTokens must be a non-negative integer (got -1)",
        ),
        (
            json!(1.5),
            "spill-policy: maxInlineTokens must be a non-negative integer (got 1.5)",
        ),
        (
            json!(9_007_199_254_740_992_u64),
            "spill-policy: maxInlineTokens must be a non-negative integer (got 9007199254740992)",
        ),
    ] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let error = ctx
            .plugin::<SpillPolicy>(json!({ "maxInlineTokens": cap }))
            .unwrap()
            .settled()
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_explicit_null_cap_instead_of_treating_it_as_omitted() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = match ctx.plugin::<SpillPolicy>(json!({ "maxInlineTokens": null })) {
        Ok(plugin) => plugin.settled().await.unwrap_err(),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "spill-policy: maxInlineTokens must be a non-negative integer (got null)"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn spills_full_text_and_bounds_the_model_facing_projection() {
    let stack = setup(json!({ "maxInlineTokens": 64 }), true).await;
    let body = format!("{}{}", "HEAD".repeat(200), "TAIL".repeat(200));
    register(&stack, "big", vec![ContentBlock::text(&body)]);
    let result = execute(&stack, "big", true).await;
    assert!(!result.is_error());
    {
        let saves = lock(&stack.store.as_ref().unwrap().saves);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].content, body);
        assert_eq!(saves[0].owner.session_id.as_ref(), "s1");
        assert_eq!(saves[0].suggested_name, "big.txt");
        assert!(matches!(
            &saves[0].source,
            hoh_spill::SpillSource::Tool { tool_name, .. } if tool_name == "big"
        ));
    }
    let text = text_of(result.content());
    assert!(text.starts_with("HEAD"));
    assert!(text.contains("Full formatted result stored at: /spill/big.txt"));
    assert!(text.contains("Omitted"));
    assert!(text.contains("Use the stub retrieval path."));
    assert!(hoh_token_meter::estimate_content(result.content()) <= 64);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_notice_too_large_small_unsupported_and_read_results_inline() {
    let tiny = setup(json!({ "maxInlineTokens": 4 }), true).await;
    register(&tiny, "big", vec![ContentBlock::text("xxxxx")]);
    assert_eq!(
        text_of(execute(&tiny, "big", true).await.content()),
        "xxxxx"
    );
    assert_eq!(
        warning_messages(&tiny),
        [
            "spill-policy: Error: spill notice for big exceeds maxInlineTokens; keeping the inline content"
        ]
    );
    tiny.kernel.shutdown().await;

    let stack = setup(json!({ "maxInlineTokens": 10 }), true).await;
    register(&stack, "small", vec![ContentBlock::text("tiny")]);
    register(
        &stack,
        "mixed",
        vec![
            ContentBlock::text("x".repeat(100)),
            ContentBlock::reasoning("why"),
        ],
    );
    register(&stack, "read", vec![ContentBlock::text("x".repeat(1_000))]);
    assert_eq!(
        text_of(execute(&stack, "small", true).await.content()),
        "tiny"
    );
    assert_eq!(execute(&stack, "mixed", true).await.content().len(), 2);
    assert_eq!(
        text_of(execute(&stack, "read", true).await.content()),
        "x".repeat(1_000)
    );
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn storage_failures_missing_backend_and_missing_owner_are_best_effort() {
    let failed = setup(json!({ "maxInlineTokens": 10 }), true).await;
    failed
        .store
        .as_ref()
        .unwrap()
        .fail
        .store(true, Ordering::SeqCst);
    register(&failed, "big", vec![ContentBlock::text("x".repeat(1_000))]);
    let result = execute(&failed, "big", true).await;
    assert!(!result.is_error());
    assert_eq!(text_of(result.content()), "x".repeat(1_000));
    assert_eq!(
        warning_messages(&failed),
        ["spill-policy: Error: disk full; keeping the inline content"]
    );
    failed.kernel.shutdown().await;

    let absent = setup(json!({ "maxInlineTokens": 10 }), false).await;
    register(&absent, "big", vec![ContentBlock::text("x".repeat(1_000))]);
    assert_eq!(
        text_of(execute(&absent, "big", true).await.content()),
        "x".repeat(1_000)
    );
    assert_eq!(
        warning_messages(&absent),
        ["spill-policy: Error: no ctx.spillStore backend loaded; keeping the inline content"]
    );
    absent.kernel.shutdown().await;

    let ownerless = setup(json!({ "maxInlineTokens": 10 }), true).await;
    register(
        &ownerless,
        "big",
        vec![ContentBlock::text("x".repeat(1_000))],
    );
    assert_eq!(
        text_of(execute(&ownerless, "big", false).await.content()),
        "x".repeat(1_000)
    );
    assert!(lock(&ownerless.store.as_ref().unwrap().saves).is_empty());
    assert_eq!(
        warning_messages(&ownerless),
        ["spill-policy: Error: no session owner for big result; keeping the inline content"]
    );
    ownerless.kernel.shutdown().await;
}

#[tokio::test]
async fn notice_fallback_preserves_a_result_far_above_the_cap() {
    let stack = setup(json!({ "maxInlineTokens": 8 }), true).await;
    let body = "x".repeat(5_000);
    register(&stack, "big", vec![ContentBlock::text(&body)]);
    assert_eq!(text_of(execute(&stack, "big", true).await.content()), body);
    assert_eq!(
        warning_messages(&stack),
        [
            "spill-policy: Error: spill notice for big exceeds maxInlineTokens; keeping the inline content"
        ]
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn bounds_downstream_content_and_preserves_contexts() {
    let stack = setup(json!({ "maxInlineTokens": 64 }), true).await;
    let context = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("note")],
        source: MessageSource::new("test"),
    });
    let expected = context.clone();
    stack
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _| {
            let context = context.clone();
            async move {
                Ok(PostToolDecision::Accept {
                    content: Some(vec![ContentBlock::text("z".repeat(500))]),
                    value: None,
                    additional_contexts: Some(vec![context]),
                })
            }
        })
        .unwrap();
    register(&stack, "small", vec![ContentBlock::text("tiny")]);
    let result = execute(&stack, "small", true).await;
    assert!(text_of(result.content()).contains("Full formatted result stored at"));
    assert_eq!(result.additional_contexts(), Some(&vec![expected]));
    assert_eq!(
        lock(&stack.store.as_ref().unwrap().saves)[0].content,
        "z".repeat(500)
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn policy_delegates_before_bounding_a_tool_owned_projection() {
    let stack = setup(json!({ "maxInlineTokens": 64 }), true).await;
    let observed = Arc::new(Mutex::new(None::<PostToolDecision>));
    let target = observed.clone();
    stack
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, next| {
            let target = target.clone();
            async move {
                let decision = next.run().await?;
                *lock(&target) = Some(decision.clone());
                Ok(PostToolDecision::Accept {
                    content: Some(vec![ContentBlock::text(format!(
                        "first page\n\nFull canonical result stored at /spill/search-results.txt.\n{}",
                        "z".repeat(500)
                    ))]),
                    value: None,
                    additional_contexts: None,
                })
            }
        })
        .unwrap();
    register(
        &stack,
        "search",
        vec![ContentBlock::text("initial capped page")],
    );
    let result = execute(&stack, "search", true).await;
    assert_eq!(*lock(&observed), Some(PostToolDecision::accept()));
    assert!(
        lock(&stack.store.as_ref().unwrap().saves)[0]
            .content
            .contains("Full canonical result stored at /spill/search-results.txt.")
    );
    assert!(text_of(result.content()).contains("Full formatted result stored at"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn accepted_value_replacement_bypasses_presentation_policy() {
    let stack = setup(json!({ "maxInlineTokens": 10 }), true).await;
    let replacement = vec![ContentBlock::text("z".repeat(500))];
    let value = serde_json::to_value(&replacement).unwrap();
    stack
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _| {
            let value = value.clone();
            async move {
                Ok(PostToolDecision::Accept {
                    content: None,
                    value: Some(value),
                    additional_contexts: None,
                })
            }
        })
        .unwrap();
    register(&stack, "small", vec![ContentBlock::text("tiny")]);
    let result = execute(&stack, "small", true).await;
    assert_eq!(text_of(result.content()), "z".repeat(500));
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn disposal_removes_both_policy_listeners() {
    let stack = setup(json!({ "maxInlineTokens": 64 }), true).await;
    let exec = seed_execution(&stack).await;
    let body = format!("{}{}", "HEAD".repeat(200), "TAIL".repeat(200));
    register(&stack, "big", vec![ContentBlock::text(&body)]);
    assert!(text_of(execute(&stack, "big", true).await.content()).contains("Full formatted"));
    stack.policy.dispose().await;
    assert_eq!(text_of(execute(&stack, "big", true).await.content()), body);
    let dispatch_content = vec![ContentBlock::text("H".repeat(2_000))];
    assert_eq!(
        dispatch_log(
            &stack,
            exec,
            "huge_dispatch",
            "parent:ptc:disposed",
            dispatch_content.clone(),
        )
        .await,
        dispatch_content
    );
    assert_eq!(lock(&stack.store.as_ref().unwrap().saves).len(), 1);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn multimodal_spill_keeps_ordered_end_images_and_readable_recovery_addresses() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    std::fs::write(&path, b"png").unwrap();
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    register(
        &stack,
        "inspect",
        vec![
            ContentBlock::text("A"),
            image('a'),
            ContentBlock::text("B".repeat(4_000)),
            image('b'),
            ContentBlock::text("C"),
        ],
    );
    let result = execute(&stack, "inspect", true).await;
    assert!(!result.is_error());
    assert_eq!(
        result
            .content()
            .iter()
            .filter(|block| block.r#type == "image")
            .count(),
        2
    );
    assert_eq!(
        result
            .content()
            .iter()
            .map(|block| block.r#type.as_str())
            .collect::<Vec<_>>(),
        ["text", "image", "text", "image", "text"]
    );
    {
        let saves = lock(&stack.store.as_ref().unwrap().saves);
        let encoded_path = serde_json::to_string(&path.to_string_lossy()).unwrap();
        let exact_address =
            format!("\n[Image: {encoded_path}; image/png; 1x1. Use read_image to view it.]\n");
        assert!(saves[0].content.contains(&"B".repeat(4_000)));
        assert_eq!(saves[0].content.matches(&exact_address).count(), 2);
    }
    assert!(
        hoh_token_meter::estimate_content(
            &result
                .content()
                .iter()
                .filter(|block| block.r#type == "text")
                .cloned()
                .collect::<Vec<_>>()
        ) + 20
            <= 200
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn multimodal_retention_preserves_the_tools_canonical_value() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    std::fs::write(&path, b"png").unwrap();
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    let original_content = vec![
        ContentBlock::text("A".repeat(4_000)),
        image('a'),
        ContentBlock::text("C".repeat(4_000)),
    ];
    let original_value = json!({ "content": original_content });
    let returned_value = original_value.clone();
    let definition = define_tool(DefineToolOptions::new(
        "inspect_value",
        "inspect value",
        json!({}),
        DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "content": {
                        "type": "array",
                        "required": true,
                        "items": { "type": "json" }
                    }
                }
            }),
            render: Arc::new(|_, value| {
                serde_json::from_value(value["content"].clone()).expect("content blocks")
            }),
            presentation_meta: None,
        },
        move |_, _| {
            let value = returned_value.clone();
            Box::pin(async move { Ok(value) })
        },
    ))
    .unwrap();
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .register(&stack.ctx, definition, ToolRegistrationOptions::default())
        .unwrap();

    let result = execute(&stack, "inspect_value", true).await;
    assert_eq!(result.value(), Some(&original_value));
    assert_ne!(
        serde_json::to_value(result.content()).unwrap(),
        original_value["content"]
    );
    assert!(text_of(result.content()).contains("Omitted 1 images."));
    assert!(hoh_token_meter::estimate_content(result.content()) <= 200);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn multimodal_spill_omits_a_middle_image_as_a_whole() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    register(
        &stack,
        "inspect",
        vec![
            ContentBlock::text("A".repeat(4_000)),
            image('a'),
            ContentBlock::text("C".repeat(4_000)),
        ],
    );
    let result = execute(&stack, "inspect", true).await;
    assert!(result.content().iter().all(|block| block.r#type == "text"));
    assert!(text_of(result.content()).contains("Omitted 1 images."));
    assert!(
        lock(&stack.store.as_ref().unwrap().saves)[0]
            .content
            .contains("[Image: ")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn missing_path_pricing_and_inconsistent_pricing_preserve_images_and_log_exact_reasons() {
    type VisualCase = (Option<Arc<dyn LlmImageRequestPricing>>, bool, bool, String);
    let cases: [VisualCase; 3] = [
        (
            Some(Arc::new(TestPricing {
                inconsistent: false,
            })),
            true,
            false,
            format!(
                "spill-policy: Error: image sha256:{} has no readable attachment path; keeping the inline content",
                "a".repeat(64)
            ),
        ),
        (
            None,
            true,
            true,
            "spill-policy: Error: the current model has no image token calculator; keeping the inline content".into(),
        ),
        (
            Some(Arc::new(TestPricing { inconsistent: true })),
            true,
            true,
            "spill-policy: Error: image token calculator returned an inconsistent occurrence count; keeping the inline content".into(),
        ),
    ];
    for (pricing, attachments, fs, warning) in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
        mount_visual_services(
            &stack,
            path.to_string_lossy().into_owned(),
            pricing,
            attachments,
            fs,
        )
        .await;
        register(
            &stack,
            "inspect",
            vec![ContentBlock::text("A".repeat(4_000)), image('a')],
        );
        let result = execute(&stack, "inspect", true).await;
        assert!(result.content().iter().any(|block| block.r#type == "image"));
        assert!(
            warning_messages(&stack)
                .iter()
                .any(|message| message == &warning),
            "missing exact warning {warning:?}"
        );
        stack.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn image_without_an_agent_route_stays_inline_and_reports_missing_pricing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    register(
        &stack,
        "inspect",
        vec![ContentBlock::text("A".repeat(4_000)), image('a')],
    );
    let result = execute(&stack, "inspect", false).await;
    assert!(result.content().iter().any(|block| block.r#type == "image"));
    assert!(warning_messages(&stack).iter().any(|message| {
        message
            == "spill-policy: Error: the current model has no image token calculator; keeping the inline content"
    }));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn partial_request_routes_fall_back_one_field_at_a_time() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    std::fs::write(&path, b"png").unwrap();
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    register(
        &stack,
        "inspect",
        vec![ContentBlock::text("A".repeat(4_000)), image('a')],
    );
    for (id, header, provider, model) in [
        (
            "partial-provider",
            json!({ "provider": "vision" }),
            "other",
            "vision",
        ),
        (
            "partial-model",
            json!({ "model": "vision" }),
            "vision",
            "other",
        ),
    ] {
        let agent = TestAgent::with_route(&stack.ctx, id, Some(provider), Some(model));
        agent
            .session
            .append(
                "request/header",
                json!({ "header": { "config": header }, "reason": "test" }),
                None,
            )
            .unwrap();
        let result = execute_as(&stack, "inspect", agent).await;
        assert!(text_of(result.content()).contains("Full formatted result stored at"));
    }
    assert_eq!(lock(&stack.store.as_ref().unwrap().saves).len(), 2);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn ptc_dispatch_log_is_bounded_without_changing_the_program_copy() {
    let stack = setup(json!({ "maxInlineTokens": 64 }), true).await;
    let exec = seed_execution(&stack).await;
    let complete = vec![ContentBlock::text("H".repeat(2_000))];
    let logged = dispatch_log(&stack, exec, "huge_read", "parent:ptc:1", complete.clone()).await;
    assert_eq!(text_of(&complete), "H".repeat(2_000));
    assert!(text_of(&logged).contains("Full formatted result stored at"));
    {
        let saves = lock(&stack.store.as_ref().unwrap().saves);
        let save = saves.last().unwrap();
        assert_eq!(save.content, "H".repeat(2_000));
        assert!(matches!(
            &save.source,
            hoh_spill::SpillSource::Tool { label, call_id, .. }
                if label == "dispatch" && call_id.as_ref() == "parent:ptc:1"
        ));
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn ptc_dispatch_log_keeps_non_text_content() {
    let stack = setup(json!({ "maxInlineTokens": 5 }), true).await;
    let exec = seed_execution(&stack).await;
    let complete = vec![
        ContentBlock::text("x".repeat(100)),
        ContentBlock::reasoning("why"),
    ];
    assert_eq!(
        dispatch_log(&stack, exec, "mixed", "parent:ptc:mixed", complete.clone(),).await,
        complete
    );
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    assert!(warning_messages(&stack).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn ptc_dispatch_log_within_cap_is_unchanged() {
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    let exec = seed_execution(&stack).await;
    let complete = vec![ContentBlock::text("tiny")];
    assert_eq!(
        dispatch_log(&stack, exec, "small", "parent:ptc:small", complete.clone(),).await,
        complete
    );
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    assert!(warning_messages(&stack).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn ptc_dispatch_log_save_failure_keeps_full_content() {
    let stack = setup(json!({ "maxInlineTokens": 40 }), true).await;
    stack
        .store
        .as_ref()
        .unwrap()
        .fail
        .store(true, Ordering::SeqCst);
    let exec = seed_execution(&stack).await;
    let complete = vec![ContentBlock::text("H".repeat(2_000))];
    assert_eq!(
        dispatch_log(&stack, exec, "huge", "parent:ptc:failure", complete.clone(),).await,
        complete
    );
    assert!(lock(&stack.store.as_ref().unwrap().saves).is_empty());
    assert_eq!(
        warning_messages(&stack),
        ["spill-policy: Error: disk full; keeping the inline content"]
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn nested_text_skips_retention_and_only_successful_images_add_ptc_context() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    let stack = setup(json!({ "maxInlineTokens": 200 }), true).await;
    mount_visual_services(
        &stack,
        path.to_string_lossy().into_owned(),
        Some(Arc::new(TestPricing {
            inconsistent: false,
        })),
        true,
        true,
    )
    .await;
    let captured = Arc::new(Mutex::new(None::<Arc<ToolExecution>>));
    let target = captured.clone();
    let fixture = define_content_tool_fixture(ContentToolFixtureOptions::new(
        "seed",
        "seed",
        json!({}),
        move |_, exec| {
            *lock(&target) = Some(exec);
            async { Ok(vec![ContentBlock::text("tiny")]) }
        },
    ))
    .unwrap();
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .register(&stack.ctx, fixture, ToolRegistrationOptions::default())
        .unwrap();
    execute(&stack, "seed", true).await;
    let outer = lock(&captured).clone().unwrap();

    register(
        &stack,
        "nested_text",
        vec![ContentBlock::text("x".repeat(2_000))],
    );
    let mut text_input = ToolExecutionInput::new(
        "nested-text".into(),
        "nested_text",
        json!({}),
        AbortController::new().signal(),
    );
    text_input.parent = Some(outer.token);
    text_input.agent = outer.agent.clone();
    let text_result = execute_tool_fixture(&stack.ctx, text_input).await.unwrap();
    assert_eq!(text_of(text_result.content()), "x".repeat(2_000));

    register(
        &stack,
        "nested_image",
        vec![
            ContentBlock::text("A".repeat(4_000)),
            image('a'),
            ContentBlock::text("C".repeat(4_000)),
        ],
    );
    let mut image_input = ToolExecutionInput::new(
        "nested-image".into(),
        "nested_image",
        json!({}),
        AbortController::new().signal(),
    );
    image_input.parent = Some(outer.token);
    image_input.agent = outer.agent.clone();
    let image_result = execute_tool_fixture(&stack.ctx, image_input).await.unwrap();
    assert!(
        image_result
            .content()
            .iter()
            .all(|block| block.r#type == "text")
    );
    let contexts = image_result.additional_contexts().unwrap();
    assert_eq!(contexts.len(), 1);
    assert!(
        contexts[0]
            .content
            .iter()
            .all(|block| block.r#type == "text")
    );
    assert!(text_of(&contexts[0].content).contains("Omitted 1 images."));

    stack
        .ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| async move {
            let result = next.run().await?;
            if event.exec.name != "nested_failure" {
                return Ok(result);
            }
            Ok(ToolExecutionResult::Failure(ToolExecutionFailure::new(
                ToolFailure {
                    message: "capture failed".into(),
                    info: None,
                },
                vec![
                    ContentBlock::text("A".repeat(4_000)),
                    image('b'),
                    ContentBlock::text("C".repeat(4_000)),
                ],
            )))
        })
        .unwrap();
    register(
        &stack,
        "nested_failure",
        vec![ContentBlock::text(
            "body is replaced by the execute wrapper",
        )],
    );
    let mut failure_input = ToolExecutionInput::new(
        "nested-failure".into(),
        "nested_failure",
        json!({}),
        AbortController::new().signal(),
    );
    failure_input.parent = Some(outer.token);
    failure_input.agent = outer.agent.clone();
    let failure_result = execute_tool_fixture(&stack.ctx, failure_input)
        .await
        .unwrap();
    assert!(failure_result.is_error());
    assert!(failure_result.additional_contexts().is_none());
    assert!(
        failure_result
            .content()
            .iter()
            .all(|block| block.r#type == "text")
    );
    assert!(text_of(failure_result.content()).contains("Omitted 1 images."));
    stack.kernel.shutdown().await;
}
