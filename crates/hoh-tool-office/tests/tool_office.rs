use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentDetach, AgentDisposed, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents,
    CancelOptions, Inbox, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, AttachmentId, AttachmentStore, Attachments,
    ImageAttachmentLimits, ImageAttachmentRef, ImageMediaType, SaveImageAttachment,
    StoredImageAttachment,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_fs::Fs;
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    GenerateOptions, Llm, LlmAdapter, LlmModelInfo, LlmResolvedModelInfo, LlmRuntime, LlmStream,
    MessageId, UserMessage,
};
use hoh_office_kit::{
    OfficeConversionRequest, OfficeFileResult, OfficeImage, OfficeImagesRequest,
    OfficeImagesResult, OfficeKit, OfficeKitApi, OfficeKitError, OfficeKitErrorCode,
    OfficeKitGeneration, OfficeKitRequest, OfficeKitResult, OfficeRecalculationRequest,
};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::{
    AgentCancelCause, DispatchValidation, SESSION_FORMAT_VERSION, Session, SessionEventEmitted,
    SessionId, SurfaceIntent,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_office::{
    ToolOffice,
    shim::{RootSessionResolver, Subagents},
};
use hoh_tools::{
    ModelSelection as ToolModelSelection, ScheduledToolDispatch, ScheduledToolPreparation,
    ToolActivationContext, ToolExecutionInput, ToolExecutionPipeline, ToolExecutionResult,
    ToolRuntime, Tools,
};
use hoh_working_directory::WorkingDirectory;
use serde_json::{Value, json};

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

struct LiveAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
}

impl Agent for LiveAgent {
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedConversion {
    source: Vec<u8>,
    owner: String,
    name: String,
}

struct FakeKit {
    output_directory: PathBuf,
    conversions: Arc<Mutex<Vec<CapturedConversion>>>,
    recalculations: Arc<Mutex<Vec<CapturedConversion>>>,
    renders: Arc<Mutex<Vec<CapturedRender>>>,
    conversion_error: Arc<Mutex<Option<OfficeKitErrorCode>>>,
    render_png_len: Arc<Mutex<usize>>,
    source_read_mode: Arc<Mutex<Option<SourceReadMode>>>,
    cancel_operation: Arc<Mutex<Option<&'static str>>>,
    cancelled_operations: Arc<Mutex<Vec<&'static str>>>,
}

enum SourceReadMode {
    Change(PathBuf),
    LimitToOneByte,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedRender {
    source: Vec<u8>,
    pages: Option<Vec<u32>>,
    dpi: Option<u32>,
    max_images: u64,
    max_pixels: u64,
    max_dimension: u64,
    max_bytes: u64,
}

impl FakeKit {
    fn unused<T>() -> hoh_office_kit::OfficeFuture<T> {
        Box::pin(async {
            Err(SharedError::new(
                Box::<dyn std::error::Error + Send + Sync>::from("unused fake Office operation"),
            ))
        })
    }
}

impl OfficeKitApi for FakeKit {
    fn generation(&self) -> OfficeKitGeneration {
        OfficeKitGeneration::from("test-generation")
    }

    fn output_directory(&self) -> &Path {
        &self.output_directory
    }

    fn convert(
        &self,
        _: OfficeKitRequest,
        _: Option<AbortSignal>,
    ) -> hoh_office_kit::OfficeFuture<OfficeKitResult> {
        Self::unused()
    }

    fn render_images(
        &self,
        request: OfficeImagesRequest,
        signal: Option<AbortSignal>,
    ) -> hoh_office_kit::OfficeFuture<OfficeImagesResult> {
        let renders = self.renders.clone();
        let render_png_len = self.render_png_len.clone();
        let source_read_mode = self.source_read_mode.clone();
        let cancel_operation = self.cancel_operation.clone();
        let cancelled_operations = self.cancelled_operations.clone();
        Box::pin(async move {
            let signal = signal.unwrap_or_else(|| AbortController::new().signal());
            if cancel_operation.lock().unwrap().take() == Some("render") {
                let reason = signal.cancelled().await;
                cancelled_operations.lock().unwrap().push("render");
                return Err(reason);
            }
            let mode = source_read_mode.lock().unwrap().take();
            if let Some(SourceReadMode::Change(path)) = &mode {
                std::fs::write(path, b"changed presentation bytes").unwrap();
            }
            let max_bytes = if matches!(mode, Some(SourceReadMode::LimitToOneByte)) {
                1
            } else {
                1024 * 1024
            };
            let source = (request.source.read)(signal, max_bytes)
                .await
                .map_err(SharedError::new)?;
            renders.lock().unwrap().push(CapturedRender {
                source: source.bytes,
                pages: request.pages,
                dpi: request.dpi,
                max_images: request.max_images,
                max_pixels: request.max_pixels,
                max_dimension: request.max_dimension,
                max_bytes: request.max_bytes,
            });
            Ok(OfficeImagesResult {
                images: vec![OfficeImage {
                    png: vec![1; *render_png_len.lock().unwrap()],
                    width: 640,
                    height: 480,
                    page: Some(2),
                    sheet: None,
                    range: None,
                }],
                page_count: 4,
                dpi: 96,
                missing_fonts: vec!["Aptos".into()],
            })
        })
    }

    fn convert_to(
        &self,
        request: OfficeConversionRequest,
        signal: Option<AbortSignal>,
    ) -> hoh_office_kit::OfficeFuture<OfficeFileResult> {
        let output_directory = self.output_directory.clone();
        let conversions = self.conversions.clone();
        let conversion_error = self.conversion_error.clone();
        let cancel_operation = self.cancel_operation.clone();
        let cancelled_operations = self.cancelled_operations.clone();
        Box::pin(async move {
            let signal = signal.unwrap_or_else(|| AbortController::new().signal());
            if cancel_operation.lock().unwrap().take() == Some("convert") {
                let reason = signal.cancelled().await;
                cancelled_operations.lock().unwrap().push("convert");
                return Err(reason);
            }
            if let Some(code) = conversion_error.lock().unwrap().take() {
                return Err(SharedError::new(Box::new(OfficeKitError::new(
                    code,
                    "Conversion stopped.",
                )) as BoxError));
            }
            let source = (request.source.read)(signal, 1024 * 1024)
                .await
                .map_err(SharedError::new)?;
            conversions.lock().unwrap().push(CapturedConversion {
                source: source.bytes,
                owner: request.output.owner,
                name: request.output.name.clone(),
            });
            Ok(OfficeFileResult {
                path: output_directory.join(request.output.name),
                bytes: 321,
                page_count: Some(2),
                missing_fonts: vec!["Missing Sans".into()],
            })
        })
    }

    fn recalculate(
        &self,
        request: OfficeRecalculationRequest,
        signal: Option<AbortSignal>,
    ) -> hoh_office_kit::OfficeFuture<OfficeFileResult> {
        let output_directory = self.output_directory.clone();
        let recalculations = self.recalculations.clone();
        let cancel_operation = self.cancel_operation.clone();
        let cancelled_operations = self.cancelled_operations.clone();
        Box::pin(async move {
            let signal = signal.unwrap_or_else(|| AbortController::new().signal());
            if cancel_operation.lock().unwrap().take() == Some("recalculate") {
                let reason = signal.cancelled().await;
                cancelled_operations.lock().unwrap().push("recalculate");
                return Err(reason);
            }
            let source = (request.source.read)(signal, 1024 * 1024)
                .await
                .map_err(SharedError::new)?;
            recalculations.lock().unwrap().push(CapturedConversion {
                source: source.bytes,
                owner: request.output.owner,
                name: request.output.name.clone(),
            });
            Ok(OfficeFileResult {
                path: output_directory.join(request.output.name),
                bytes: 123,
                page_count: Some(1),
                missing_fonts: Vec::new(),
            })
        })
    }

    fn release(&self, _: String) -> hoh_office_kit::OfficeFuture<()> {
        Box::pin(async { Ok(()) })
    }
}

struct FakeAttachments {
    limits: ImageAttachmentLimits,
    saved: Arc<Mutex<Vec<SaveImageAttachment>>>,
    stored_media_type: Arc<Mutex<ImageMediaType>>,
    save_failure: Arc<Mutex<Option<bool>>>,
}

impl AttachmentStore for FakeAttachments {
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
            if let Some(classified) = self.save_failure.lock().unwrap().take() {
                if classified {
                    let error: BoxError = Box::new(AttachmentError::new(
                        "invalid rendered image",
                        AttachmentErrorCode::InvalidImage,
                    ));
                    return Err(error);
                }
                let error: BoxError = "attachment disk failure".into();
                return Err(error);
            }
            self.saved.lock().unwrap().push(input.clone());
            Ok(ImageAttachmentRef {
                attachment_id: AttachmentId::from("stored-render"),
                media_type: *self.stored_media_type.lock().unwrap(),
                bytes: 2,
                width: 320,
                height: 240,
                name: input.name.clone(),
                original_dimensions: Some(hoh_attachment::ProjectedDimensions {
                    width: 640,
                    height: 480,
                }),
            })
        })
    }

    fn read_image<'a>(
        &'a self,
        _: &'a ImageAttachmentRef,
        _: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err("unused fake attachment read".into()) })
    }
}

struct VisualAdapter;

enum RootBehavior {
    Root(SessionId),
    Unauthorized,
    Failed,
}

struct FakeRootResolver(RootBehavior);

impl RootSessionResolver for FakeRootResolver {
    fn root_of(&self, _: &Session) -> Result<SessionId, BoxError> {
        match &self.0 {
            RootBehavior::Root(id) => Ok(id.clone()),
            RootBehavior::Unauthorized => Err(Box::new(HarnessError::named(
                "SubagentError",
                "agent has no active runtime owner",
                "UNAUTHORIZED",
            ))),
            RootBehavior::Failed => Err(Box::new(HarnessError::named(
                "SubagentError",
                "subagent tree is unavailable",
                "UNSUPPORTED_CAPABILITY",
            ))),
        }
    }
}

impl LlmAdapter for VisualAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _: Option<&'a AbortSignal>,
    ) -> hoh_llm::LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    input_modalities: match model {
                        "text-only-model" => Some(vec!["text".into()]),
                        "no-modalities-model" => None,
                        _ => Some(vec!["text".into(), "image".into()]),
                    },
                    ..LlmModelInfo::default()
                },
                ..LlmResolvedModelInfo::default()
            })
        })
    }

    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        Err("unused fake LLM stream".into())
    }
}

struct Stack {
    kernel: Kernel,
    ctx: Context,
    conversions: Arc<Mutex<Vec<CapturedConversion>>>,
    recalculations: Arc<Mutex<Vec<CapturedConversion>>>,
    renders: Arc<Mutex<Vec<CapturedRender>>>,
    saved_images: Arc<Mutex<Vec<SaveImageAttachment>>>,
    conversion_error: Arc<Mutex<Option<OfficeKitErrorCode>>>,
    render_png_len: Arc<Mutex<usize>>,
    stored_media_type: Arc<Mutex<ImageMediaType>>,
    save_failure: Arc<Mutex<Option<bool>>>,
    source_read_mode: Arc<Mutex<Option<SourceReadMode>>>,
    cancel_operation: Arc<Mutex<Option<&'static str>>>,
    cancelled_operations: Arc<Mutex<Vec<&'static str>>>,
    _adapter: Option<hoh_llm::AdapterRegistrationHandle>,
}

async fn mount(root: &Path, with_kit: bool) -> Stack {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fs = Arc::new(
        LocalFileSystem::new(LocalConfig {
            cwd: Some(root.to_string_lossy().into_owned()),
            ..LocalConfig::default()
        })
        .unwrap(),
    );
    ctx.inject(move |ctx, (): ()| {
        let fs = fs.clone();
        async move {
            ctx.provide::<Fs>(fs).await?;
            Ok(())
        }
    })
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
    ctx.plugin::<WorkingDirectory>(json!({ "defaultDirectory": root.to_string_lossy() }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let conversions = Arc::new(Mutex::new(Vec::new()));
    let recalculations = Arc::new(Mutex::new(Vec::new()));
    let renders = Arc::new(Mutex::new(Vec::new()));
    let saved_images = Arc::new(Mutex::new(Vec::new()));
    let conversion_error = Arc::new(Mutex::new(None));
    let render_png_len = Arc::new(Mutex::new(3));
    let stored_media_type = Arc::new(Mutex::new(ImageMediaType::Webp));
    let save_failure = Arc::new(Mutex::new(None));
    let source_read_mode = Arc::new(Mutex::new(None));
    let cancel_operation = Arc::new(Mutex::new(None));
    let cancelled_operations = Arc::new(Mutex::new(Vec::new()));
    let mut adapter = None;
    if with_kit {
        let service = Arc::new(FakeKit {
            output_directory: root.join("office-outputs"),
            conversions: conversions.clone(),
            recalculations: recalculations.clone(),
            renders: renders.clone(),
            conversion_error: conversion_error.clone(),
            render_png_len: render_png_len.clone(),
            source_read_mode: source_read_mode.clone(),
            cancel_operation: cancel_operation.clone(),
            cancelled_operations: cancelled_operations.clone(),
        });
        ctx.inject(move |ctx, (): ()| {
            let service = service.clone();
            async move {
                ctx.provide::<OfficeKit>(service).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
        let attachments: Arc<dyn AttachmentStore> = Arc::new(FakeAttachments {
            limits: ImageAttachmentLimits {
                max_image_bytes: 1_000,
                max_images_per_message: 3,
                max_message_image_bytes: 2_000,
                max_image_pixels: 500_000,
                max_image_dimension: 1_000,
                media_types: vec![
                    ImageMediaType::Png,
                    ImageMediaType::Jpeg,
                    ImageMediaType::Webp,
                ],
            },
            saved: saved_images.clone(),
            stored_media_type: stored_media_type.clone(),
            save_failure: save_failure.clone(),
        });
        ctx.inject(move |ctx, (): ()| {
            let attachments = attachments.clone();
            async move {
                ctx.provide::<Attachments>(attachments).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
        let runtime = Arc::new(LlmRuntime::with_context(&ctx));
        let provided = runtime.clone();
        ctx.inject(move |ctx, (): ()| {
            let runtime = provided.clone();
            async move {
                ctx.provide::<Llm>(runtime).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
        adapter = Some(
            runtime
                .register_adapter(&ctx, vec!["test".into()], Arc::new(VisualAdapter))
                .unwrap(),
        );
    }
    ctx.plugin::<ToolOffice>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    Stack {
        kernel,
        ctx,
        conversions,
        recalculations,
        renders,
        saved_images,
        conversion_error,
        render_png_len,
        stored_media_type,
        save_failure,
        source_read_mode,
        cancel_operation,
        cancelled_operations,
        _adapter: adapter,
    }
}

struct Owner {
    live: Arc<LiveAgent>,
    tool: Arc<dyn hoh_tools::Agent>,
    _scope: Scope,
    detach: AgentDetach,
}

fn owner_in(stack: &Stack, id: &str, root: &Path) -> Owner {
    let scope_key = ScopeKey::new();
    let scope = create_scope(&stack.ctx, scope_key.clone(), CreateScopeOptions::default()).unwrap();
    let id: SessionId = id.into();
    let session = Session::create(
        id.clone(),
        None,
        Some(json!({
            "version": SESSION_FORMAT_VERSION,
            "id": id,
            "createdAt": 0,
            "isSeeded": false,
            "cwd": root.to_string_lossy()
        })),
        None,
        None,
    )
    .unwrap();
    let live = Arc::new(LiveAgent {
        id,
        session: session.clone(),
        ctx: scope.ctx.clone(),
        scope: scope_key.clone(),
        options: AgentOptions::default(),
    });
    let registered: Arc<dyn Agent> = live.clone();
    let detach = stack
        .ctx
        .get::<Agents>()
        .unwrap()
        .enter(&stack.ctx, registered)
        .unwrap();
    let tool: Arc<dyn hoh_tools::Agent> = live.clone();
    Owner {
        tool,
        live,
        _scope: scope,
        detach,
    }
}

fn activate(owner: &Owner, tool: &str) {
    owner
        .live
        .session()
        .append(
            "request/header",
            json!({
                "header": {
                    "config": { "provider": "test", "model": "test-model" },
                    "tools": [{ "name": tool }]
                },
                "reason": "initial"
            }),
            None,
        )
        .unwrap();
}

async fn resolve(stack: &Stack, owner: &Owner) -> hoh_tools::ResolvedToolSet {
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .resolve(ToolActivationContext {
            agent: owner.tool.clone(),
            model_selection: ToolModelSelection {
                provider: "test".into(),
                model: "test-model".into(),
                reasoning_effort: None,
            },
            signal: AbortController::new().signal(),
        })
        .await
        .unwrap()
}

async fn call(stack: &Stack, owner: &Owner, name: &str, arguments: Value) -> ToolExecutionResult {
    call_with_signal(
        stack,
        owner,
        name,
        arguments,
        AbortController::new().signal(),
    )
    .await
}

async fn call_with_signal(
    stack: &Stack,
    owner: &Owner,
    name: &str,
    arguments: Value,
    signal: AbortSignal,
) -> ToolExecutionResult {
    let mut input = ToolExecutionInput::new(format!("{name}-call").into(), name, arguments, signal);
    input.agent = Some(owner.tool.clone());
    input.tool_set = Some(resolve(stack, owner).await);
    let pipeline = ToolExecutionPipeline::new(stack.ctx.clone()).unwrap();
    let prepared = pipeline
        .prepare(input)
        .await
        .expect("the tool lookup does not reject");
    let exec = prepared.exec().clone();
    let dispatched = match prepared {
        ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        ScheduledToolPreparation::PostResult { result, .. } => {
            ScheduledToolDispatch::PostResult(result)
        }
        ScheduledToolPreparation::FinalResult { result, .. } => {
            ScheduledToolDispatch::FinalResult(result)
        }
    };
    pipeline.complete(&exec, dispatched).await
}

fn error_code(result: &ToolExecutionResult) -> Option<&str> {
    result
        .error()
        .and_then(|failure| failure.info.as_ref())
        .map(|info| info.code.as_str())
}

#[tokio::test]
async fn one_office_tool_in_request_header_activates_all_three() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-activation", &root);
    let tools = stack.ctx.get::<Tools>().unwrap();

    let inactive = resolve(&stack, &owner).await;
    for name in ["office_render", "office_convert", "office_recalculate"] {
        assert!(
            tools
                .get(name, Some(&owner.live.scope), Some(&inactive))
                .is_none()
        );
    }

    activate(&owner, "office_convert");
    let active = resolve(&stack, &owner).await;
    for name in ["office_render", "office_convert", "office_recalculate"] {
        assert!(
            tools
                .get(name, Some(&owner.live.scope), Some(&active))
                .is_some()
        );
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn direct_skill_invocation_marks_agent_identity_and_disposal_clears_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-direct-skill", &root);
    let tools = stack.ctx.get::<Tools>().unwrap();

    let other = owner
        .live
        .session()
        .append(
            "user/message",
            json!({
                "content": [{ "type": "text", "text": "other instructions" }],
                "source": { "kind": "skill-invocation", "name": "other", "form": "instructions" },
                "role": "user",
                "id": "other-skill-message"
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    stack
        .ctx
        .dispatch(SessionEventEmitted {
            session: owner.live.session().clone(),
            event: other,
            validation: DispatchValidation::default(),
        })
        .invoke(|_| panic!("session listener failed"));
    let inactive = resolve(&stack, &owner).await;
    assert!(
        tools
            .get("office_render", Some(&owner.live.scope), Some(&inactive))
            .is_none()
    );

    let office = owner
        .live
        .session()
        .append(
            "user/message",
            json!({
                "content": [{ "type": "text", "text": "office instructions" }],
                "source": { "kind": "skill-invocation", "name": "office", "form": "instructions" },
                "role": "user",
                "id": "office-skill-message"
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    stack
        .ctx
        .dispatch(SessionEventEmitted {
            session: owner.live.session().clone(),
            event: office,
            validation: DispatchValidation::default(),
        })
        .invoke(|_| panic!("session listener failed"));
    let active = resolve(&stack, &owner).await;
    assert!(
        tools
            .get("office_render", Some(&owner.live.scope), Some(&active))
            .is_some()
    );

    let disposed_agent: Arc<dyn Agent> = owner.live.clone();
    stack
        .ctx
        .dispatch(AgentDisposed {
            agent: disposed_agent,
        })
        .invoke(|_| panic!("agent disposal listener failed"));
    let disposed = resolve(&stack, &owner).await;
    assert!(
        tools
            .get("office_render", Some(&owner.live.scope), Some(&disposed))
            .is_none()
    );
    owner.detach.detach();
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn convert_reads_source_lazily_and_returns_a_workspace_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("quarterly report.docx"), b"office source bytes").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-convert", &root);
    activate(&owner, "office_convert");

    let result = call(
        &stack,
        &owner,
        "office_convert",
        json!({ "input": "quarterly report.docx", "format": "pdf" }),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    assert_eq!(result.value().unwrap()["operation"], "convert");
    assert_eq!(result.value().unwrap()["format"], "pdf");
    assert_eq!(result.value().unwrap()["bytes"], 321);
    assert_eq!(result.value().unwrap()["pageCount"], 2);
    assert_eq!(
        result.value().unwrap()["missingFonts"],
        json!(["Missing Sans"])
    );
    assert_eq!(
        stack.conversions.lock().unwrap().as_slice(),
        &[CapturedConversion {
            source: b"office source bytes".to_vec(),
            owner: "office-convert".into(),
            name: "quarterly report.pdf".into(),
        }]
    );
    assert!(
        result.value().unwrap()["file"]
            .as_str()
            .unwrap()
            .ends_with("office-outputs/quarterly report.pdf")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_bounds_the_request_and_returns_normalized_attachment_references() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation source").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-render", &root);
    activate(&owner, "office_render");

    let result = call(
        &stack,
        &owner,
        "office_render",
        json!({ "input": "deck.pptx", "pages": [2], "dpi": 96 }),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    assert_eq!(result.value().unwrap()["operation"], "render");
    assert_eq!(result.value().unwrap()["pageCount"], 4);
    assert_eq!(result.value().unwrap()["images"][0]["page"], 2);
    assert_eq!(
        result.value().unwrap()["images"][0]["mediaType"],
        "image/webp"
    );
    assert_eq!(result.value().unwrap()["images"][0]["width"], 320);
    assert_eq!(
        result.value().unwrap()["images"][0]["originalDimensions"],
        json!({ "width": 640, "height": 480 })
    );
    assert_eq!(
        stack.renders.lock().unwrap().as_slice(),
        &[CapturedRender {
            source: b"presentation source".to_vec(),
            pages: Some(vec![2]),
            dpi: Some(96),
            max_images: 3,
            max_pixels: 500_000,
            max_dimension: 1_000,
            max_bytes: 2_000,
        }]
    );
    {
        let saved = stack.saved_images.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].data, vec![1, 1, 1]);
        assert_eq!(saved[0].media_type, ImageMediaType::Png);
        assert_eq!(saved[0].name.as_deref(), Some("deck.pptx-1.png"));
    }
    assert!(result.text().contains("Page 2: 320x240 px"));
    assert!(
        result
            .text()
            .contains("Fonts not installed and substituted: Aptos.")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_accepts_integral_float_pages_and_dpi_like_javascript() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation source").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-integral-floats", &root);
    activate(&owner, "office_render");

    let result = call(
        &stack,
        &owner,
        "office_render",
        json!({ "input": "deck.pptx", "pages": [2.0], "dpi": 144.0 }),
    )
    .await;

    assert!(!result.is_error(), "{}", result.text());
    assert_eq!(stack.renders.lock().unwrap()[0].pages, Some(vec![2]));
    assert_eq!(stack.renders.lock().unwrap()[0].dpi, Some(144));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_rejects_out_of_range_json_integer_as_invalid_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-unsafe-page", &root);
    activate(&owner, "office_render");

    let result = call(
        &stack,
        &owner,
        "office_render",
        json!({ "input": "deck.pptx", "pages": [1e19] }),
    )
    .await;

    assert!(result.is_error());
    assert_eq!(error_code(&result), Some("INVALID_ARGS"));
    assert!(
        result
            .text()
            .contains("pages must list distinct one-based page numbers")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_preserves_typescript_validation_order_for_unsafe_integers() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, false).await;
    let owner = owner_in(&stack, "office-unsafe-validation-order", &root);
    activate(&owner, "office_render");

    let cases = [
        (
            json!({ "input": "a.txt", "dpi": 1e19 }),
            "cannot render \"a.txt\": render accepts",
        ),
        (
            json!({ "input": "b.xlsx", "pages": [1e19] }),
            "pages does not apply to spreadsheets",
        ),
        (
            json!({ "input": "a.docx", "sheet": "x", "dpi": 1e19 }),
            "sheet applies only to spreadsheets",
        ),
    ];
    for (arguments, expected) in cases {
        let result = call(&stack, &owner, "office_render", arguments).await;
        assert!(result.is_error());
        assert_eq!(error_code(&result), Some("INVALID_ARGS"));
        assert!(result.text().contains(expected), "{}", result.text());
    }
    assert!(stack.renders.lock().unwrap().is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_rejects_unsafe_dpi_and_duplicates_after_float_normalization() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, false).await;
    let owner = owner_in(&stack, "office-numeric-validation", &root);
    activate(&owner, "office_render");

    for (arguments, expected) in [
        (
            json!({ "input": "a.docx", "dpi": 1e19 }),
            "dpi must be an integer from 24 to 600",
        ),
        (
            json!({ "input": "a.docx", "pages": [1.0, 1] }),
            "pages must list distinct one-based page numbers",
        ),
    ] {
        let result = call(&stack, &owner, "office_render", arguments).await;
        assert!(result.is_error());
        assert_eq!(error_code(&result), Some("INVALID_ARGS"));
        assert!(result.text().contains(expected), "{}", result.text());
    }
    assert!(stack.renders.lock().unwrap().is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn all_source_argument_refusals_run_before_office_and_filesystem_access() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, false).await;
    let owner = owner_in(&stack, "office-argument-table", &root);
    activate(&owner, "office_render");

    let cases = [
        (
            "office_render",
            json!({"input":"notes.txt"}),
            "render accepts",
        ),
        (
            "office_convert",
            json!({"input":"paper.pdf","format":"docx"}),
            "convert accepts",
        ),
        (
            "office_recalculate",
            json!({"input":"deck.pptx","format":"xlsx"}),
            "recalculate accepts",
        ),
        ("office_render", json!({"input":".pptx"}), "render accepts"),
        (
            "office_convert",
            json!({"input":"book.xlsx","format":"pdf","sheet":"A"}),
            "sheet applies only to CSV",
        ),
        (
            "office_convert",
            json!({"input":"book.xlsx","format":"csv","sheet":" "}),
            "exact worksheet",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","format":"pdf"}),
            "format applies only",
        ),
        (
            "office_convert",
            json!({"input":"deck.pptx","format":"pdf","pages":[1]}),
            "pages applies only",
        ),
        (
            "office_convert",
            json!({"input":"book.xlsx","format":"pdf","range":"A1:B2"}),
            "range applies only",
        ),
        (
            "office_convert",
            json!({"input":"deck.pptx","format":"pdf","dpi":96}),
            "dpi applies only",
        ),
        (
            "office_recalculate",
            json!({"input":"book.xlsx","format":"xlsx","sheet":"Data"}),
            "sheet applies only",
        ),
        (
            "office_recalculate",
            json!({"input":"book.xlsx","format":"xlsx","pages":[1]}),
            "pages applies only",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","output":"deck.pdf"}),
            "does not accept output",
        ),
        (
            "office_render",
            json!({"input":"book.xlsx","pages":[1]}),
            "pages does not apply",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","pages":[]}),
            "distinct one-based",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","pages":[0]}),
            "distinct one-based",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","pages":[1,1]}),
            "distinct one-based",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","sheet":"A"}),
            "sheet applies only",
        ),
        (
            "office_render",
            json!({"input":"book.xlsx","range":"A1"}),
            "range requires sheet",
        ),
        (
            "office_render",
            json!({"input":"book.xlsx","sheet":" "}),
            "exact worksheet",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","dpi":23}),
            "integer from 24 to 600",
        ),
        (
            "office_render",
            json!({"input":"deck.pptx","dpi":601}),
            "integer from 24 to 600",
        ),
        (
            "office_convert",
            json!({"input":"deck.pptx","format":"xlsx"}),
            "converts to pdf, pptx, odp",
        ),
    ];
    for (tool, arguments, expected) in cases {
        let result = call(&stack, &owner, tool, arguments).await;
        assert!(result.is_error(), "{tool} unexpectedly succeeded");
        assert_eq!(
            error_code(&result),
            Some("INVALID_ARGS"),
            "{}",
            result.text()
        );
        assert!(result.text().contains(expected), "{}", result.text());
        assert!(
            result.text().starts_with("Error: invalid arguments:"),
            "{}",
            result.text()
        );
    }
    assert!(stack.conversions.lock().unwrap().is_empty());
    assert!(stack.recalculations.lock().unwrap().is_empty());
    assert!(stack.renders.lock().unwrap().is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn source_conversion_matrix_and_recalculation_reach_the_kit() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    for name in ["report.docx", "book.xlsx", "deck.pptx"] {
        std::fs::write(root.join(name), name.as_bytes()).unwrap();
    }
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-conversion-matrix", &root);
    activate(&owner, "office_convert");

    for (input, format) in [
        ("report.docx", "pdf"),
        ("report.docx", "docx"),
        ("report.docx", "odt"),
        ("report.docx", "txt"),
        ("book.xlsx", "pdf"),
        ("book.xlsx", "xlsx"),
        ("book.xlsx", "ods"),
        ("book.xlsx", "csv"),
        ("deck.pptx", "pdf"),
        ("deck.pptx", "pptx"),
        ("deck.pptx", "odp"),
    ] {
        let result = call(
            &stack,
            &owner,
            "office_convert",
            json!({"input": input, "format": format}),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(result.value().unwrap()["format"], format);
    }
    let recalculated = call(
        &stack,
        &owner,
        "office_recalculate",
        json!({"input":"book.xlsx","format":"ods"}),
    )
    .await;
    assert!(!recalculated.is_error(), "{}", recalculated.text());
    assert_eq!(recalculated.value().unwrap()["operation"], "recalculate");
    assert_eq!(stack.conversions.lock().unwrap().len(), 11);
    assert_eq!(stack.recalculations.lock().unwrap().len(), 1);
    assert_eq!(stack.recalculations.lock().unwrap()[0].name, "book.ods");
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn output_owner_uses_root_fallback_and_passthrough_from_fake_resolver() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();

    for (suffix, behavior, expected_owner, expected_error) in [
        (
            "root",
            RootBehavior::Root(SessionId::from("root-session")),
            Some("root-session"),
            None,
        ),
        (
            "unauthorized",
            RootBehavior::Unauthorized,
            Some("owner-unauthorized"),
            None,
        ),
        (
            "failed",
            RootBehavior::Failed,
            None,
            Some("UNSUPPORTED_CAPABILITY"),
        ),
    ] {
        let stack = mount(&root, true).await;
        let service: Arc<dyn RootSessionResolver> = Arc::new(FakeRootResolver(behavior));
        let _provider = stack.ctx.provide::<Subagents>(service).await.unwrap();
        let id = format!("owner-{suffix}");
        let owner = owner_in(&stack, &id, &root);
        activate(&owner, "office_convert");
        let result = call(
            &stack,
            &owner,
            "office_convert",
            json!({"input":"deck.pptx","format":"pdf"}),
        )
        .await;
        if let Some(code) = expected_error {
            assert!(result.is_error());
            assert_eq!(error_code(&result), Some(code));
            assert!(result.text().contains("subagent tree is unavailable"));
        } else {
            assert!(!result.is_error(), "{}", result.text());
            assert_eq!(
                stack.conversions.lock().unwrap()[0].owner,
                expected_owner.unwrap()
            );
        }
        stack.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn pipeline_classifies_every_office_conversion_failure_code() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-failure-table", &root);
    activate(&owner, "office_convert");

    for (kit_code, tool_code) in [
        (OfficeKitErrorCode::Unavailable, "OFFICE_UNAVAILABLE"),
        (OfficeKitErrorCode::Busy, "OFFICE_BUSY"),
        (OfficeKitErrorCode::InputTooLarge, "OFFICE_INPUT_TOO_LARGE"),
        (
            OfficeKitErrorCode::OutputTooLarge,
            "OFFICE_OUTPUT_TOO_LARGE",
        ),
        (
            OfficeKitErrorCode::InvalidDocument,
            "OFFICE_INVALID_DOCUMENT",
        ),
        (
            OfficeKitErrorCode::UnsupportedFormat,
            "OFFICE_UNSUPPORTED_FORMAT",
        ),
        (OfficeKitErrorCode::InvalidRequest, "OFFICE_INVALID_REQUEST"),
        (OfficeKitErrorCode::InvalidOutput, "OFFICE_INVALID_OUTPUT"),
        (OfficeKitErrorCode::Timeout, "OFFICE_TIMEOUT"),
        (OfficeKitErrorCode::SourceChanged, "OFFICE_SOURCE_CHANGED"),
        (OfficeKitErrorCode::Failed, "OFFICE_CONVERSION_FAILED"),
    ] {
        *stack.conversion_error.lock().unwrap() = Some(kit_code);
        let result = call(
            &stack,
            &owner,
            "office_convert",
            json!({"input":"deck.pptx","format":"pdf"}),
        )
        .await;
        assert!(result.is_error());
        assert_eq!(error_code(&result), Some(tool_code), "{}", result.text());
        assert!(result.text().contains("Conversion stopped."));
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn caller_cancellation_reaches_all_three_office_operations() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();
    std::fs::write(root.join("book.xlsx"), b"workbook").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-cancellation", &root);
    activate(&owner, "office_render");

    for (operation, tool, arguments) in [
        ("render", "office_render", json!({"input":"deck.pptx"})),
        (
            "convert",
            "office_convert",
            json!({"input":"deck.pptx","format":"pdf"}),
        ),
        (
            "recalculate",
            "office_recalculate",
            json!({"input":"book.xlsx","format":"xlsx"}),
        ),
    ] {
        *stack.cancel_operation.lock().unwrap() = Some(operation);
        let controller = AbortController::new();
        let future = call_with_signal(&stack, &owner, tool, arguments, controller.signal());
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("{operation} completed before cancellation: {}", result.text()),
            () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
        controller.abort_with("caller cancelled");
        let result = future.await;
        assert!(result.is_error());
        assert!(result.text().contains("caller cancelled"));
    }
    assert_eq!(
        stack.cancelled_operations.lock().unwrap().as_slice(),
        &["render", "convert", "recalculate"]
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn render_enforces_per_image_limit_and_rejects_gif_storage() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-render-storage", &root);
    activate(&owner, "office_render");

    *stack.render_png_len.lock().unwrap() = 1_001;
    let too_large = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert_eq!(error_code(&too_large), Some("OFFICE_OUTPUT_TOO_LARGE"));
    assert!(too_large.text().contains("1000-byte image limit"));

    *stack.render_png_len.lock().unwrap() = 3;
    *stack.stored_media_type.lock().unwrap() = ImageMediaType::Gif;
    let gif = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert!(gif.is_error());
    assert!(gif.text().contains("stored a rendered PNG as GIF"));

    *stack.stored_media_type.lock().unwrap() = ImageMediaType::Webp;
    *stack.save_failure.lock().unwrap() = Some(true);
    let refused = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert_eq!(error_code(&refused), Some("INVALID_IMAGE"));
    assert!(refused.text().contains("cannot render"));

    *stack.save_failure.lock().unwrap() = Some(false);
    let storage = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert!(storage.is_error());
    assert_eq!(error_code(&storage), None);
    assert!(storage.text().contains("attachment disk failure"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn deferred_source_read_reports_changed_and_grown_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let source = root.join("deck.pptx");
    std::fs::write(&source, b"presentation").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-source-integrity", &root);
    activate(&owner, "office_render");

    *stack.source_read_mode.lock().unwrap() = Some(SourceReadMode::Change(source.clone()));
    let changed = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert_eq!(error_code(&changed), Some("OFFICE_SOURCE_CHANGED"));
    assert!(changed.text().contains("The source changed."));

    *stack.source_read_mode.lock().unwrap() = Some(SourceReadMode::LimitToOneByte);
    let grown = call(
        &stack,
        &owner,
        "office_render",
        json!({"input":"deck.pptx"}),
    )
    .await;
    assert_eq!(error_code(&grown), Some("OFFICE_INPUT_TOO_LARGE"));
    assert!(grown.text().contains("reserved byte capacity"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn argument_validation_runs_before_office_kit_and_filesystem_access() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, false).await;
    let owner = owner_in(&stack, "office-invalid", &root);
    activate(&owner, "office_convert");

    let result = call(
        &stack,
        &owner,
        "office_convert",
        json!({ "input": "missing.txt", "format": "pdf" }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(error_code(&result), Some("INVALID_ARGS"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn missing_office_kit_is_reported_before_a_missing_input_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, false).await;
    let owner = owner_in(&stack, "office-unavailable", &root);
    activate(&owner, "office_convert");

    let result = call(
        &stack,
        &owner,
        "office_convert",
        json!({ "input": "missing.docx", "format": "pdf" }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(error_code(&result), Some("OFFICE_UNAVAILABLE"));
    assert!(result.text().contains("do not substitute system soffice"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn valid_request_with_a_kit_preserves_filesystem_not_found() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-not-found", &root);
    activate(&owner, "office_convert");

    let result = call(
        &stack,
        &owner,
        "office_convert",
        json!({ "input": "missing.docx", "format": "pdf" }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(error_code(&result), Some("FS_NOT_FOUND"));
    stack.kernel.shutdown().await;
}

fn activate_with_config(owner: &Owner, tool: &str, config: Value) {
    owner
        .live
        .session()
        .append(
            "request/header",
            json!({
                "header": { "config": config, "tools": [{ "name": tool }] },
                "reason": "initial"
            }),
            None,
        )
        .unwrap();
}

#[tokio::test]
async fn render_refuses_models_without_image_input_and_unresolved_routes() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();
    for (id, config, code, text) in [
        (
            "route-text",
            json!({ "provider": "test", "model": "text-only-model" }),
            Some("MODEL_DOES_NOT_SUPPORT_IMAGES"),
            "does not accept images",
        ),
        (
            "route-none",
            json!({ "provider": "test", "model": "no-modalities-model" }),
            Some("MODEL_DOES_NOT_SUPPORT_IMAGES"),
            "does not accept images",
        ),
        (
            "route-missing",
            json!({}),
            None,
            "current model route could not be resolved",
        ),
    ] {
        let stack = mount(&root, true).await;
        let owner = owner_in(&stack, id, &root);
        activate_with_config(&owner, "office_render", config);
        let result = call(
            &stack,
            &owner,
            "office_render",
            json!({"input":"deck.pptx"}),
        )
        .await;
        assert!(result.is_error(), "{id}");
        assert!(result.text().contains(text), "{id}: {}", result.text());
        if code.is_some() {
            assert_eq!(error_code(&result), code, "{id}");
        }
        assert!(stack.renders.lock().unwrap().is_empty(), "{id}");
        stack.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn render_reports_the_media_type_the_attachment_store_chose() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("deck.pptx"), b"presentation").unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "render-jpeg", &root);
    activate(&owner, "office_render");
    for (stored, expected) in [
        (ImageMediaType::Jpeg, "image/jpeg"),
        (ImageMediaType::Webp, "image/webp"),
        (ImageMediaType::Png, "image/png"),
    ] {
        *stack.stored_media_type.lock().unwrap() = stored;
        let result = call(
            &stack,
            &owner,
            "office_render",
            json!({"input":"deck.pptx"}),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(result.value().unwrap()["images"][0]["mediaType"], expected);
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn backslash_dot_name_has_no_extension_on_every_platform() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root, true).await;
    let owner = owner_in(&stack, "office-backslash", &root);
    activate(&owner, "office_render");
    let result = call(
        &stack,
        &owner,
        "office_render",
        json!({ "input": "/w/a\\.docx" }),
    )
    .await;
    assert_eq!(
        error_code(&result),
        Some("INVALID_ARGS"),
        "{}",
        result.text()
    );
    assert!(
        result.text().contains("render accepts"),
        "{}",
        result.text()
    );
    assert!(stack.renders.lock().unwrap().is_empty());
    stack.kernel.shutdown().await;
}
