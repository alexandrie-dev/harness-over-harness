//! Source-corresponding behavior tests for the `present` tool.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentDetach, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions,
    Inbox, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_fs::{
    FileSystem, Fs, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditOutcome, FsEditRequest,
    FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsPathInfo, FsResolveOptions, FsTarget,
    FsTextStream, FsVersionGuard, FsWriteIntent, FsWriteOutcome,
};
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{ContentBlock, MessageId, UserMessage};
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause, SESSION_FORMAT_VERSION, Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_present::{PresentedEvent, ToolPresent};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, ScheduledToolDispatch,
    ScheduledToolPreparation, ToolExecutionInput, ToolExecutionPipeline, ToolExecutionResult,
    ToolRegistrationOptions, ToolRuntime, Tools, ToolsPostExecute, define_tool,
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

struct ToolSeam {
    key: ScopeKey,
    session: Session,
}

impl hoh_tools::Agent for ToolSeam {
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }

    fn session(&self) -> Session {
        self.session.clone()
    }
}

struct StatOverride {
    path: String,
    info: FsInfo,
}

struct TrackingFs {
    inner: LocalFileSystem,
    byte_reads: AtomicUsize,
    stat_override: Mutex<Option<StatOverride>>,
}

impl TrackingFs {
    fn override_stat(&self, path: &Path, info: FsInfo) {
        *self.stat_override.lock().unwrap() = Some(StatOverride {
            path: path_text(path),
            info,
        });
    }
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(fn $name<'a>(&'a self, $($arg: $ty),*) -> FsFuture<'a, $ret> {
            self.inner.$name($($arg),*)
        })*
    };
}

impl FileSystem for TrackingFs {
    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        self.inner.resolve(path, options)
    }

    fn process_path(&self, target: &FsTarget) -> String {
        self.inner.process_path(target)
    }

    fn file_url(&self, target: &FsTarget) -> String {
        self.inner.file_url(target)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.inner.contains(parent, child)
    }

    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            signal
                .as_ref()
                .map(AbortSignal::throw_if_aborted)
                .transpose()?;
            let replacement = {
                let mut slot = self.stat_override.lock().unwrap();
                if slot
                    .as_ref()
                    .is_some_and(|replacement| replacement.path == target.display_path)
                {
                    slot.take().map(|replacement| replacement.info)
                } else {
                    None
                }
            };
            match replacement {
                Some(info) => Ok(Some(info)),
                None => self.inner.stat(target, signal).await,
            }
        })
    }

    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        self.inner.lstat(path, cwd, signal)
    }

    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        self.byte_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_bytes(target, signal, max_bytes)
    }

    delegate! {
        read_text(target: &'a FsTarget, signal: Option<AbortSignal>) -> String;
        stream_text(target: &'a FsTarget, signal: Option<AbortSignal>) -> FsTextStream;
        read_byte_range(target: &'a FsTarget, range: FsByteRange, signal: Option<AbortSignal>) -> Vec<u8>;
        list_dir(target: &'a FsTarget, signal: Option<AbortSignal>) -> Vec<FsDirEntry>;
        write_text(target: &'a FsTarget, content: &'a str, expected: Option<FsWriteIntent>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsWriteOutcome;
        edit_text(target: &'a FsTarget, edit: FsEditRequest, expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsEditOutcome;
        patch_text(target: &'a FsTarget, hunks: &'a [hoh_fs_patch::PatchHunk], expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsEditOutcome;
        delete_file(target: &'a FsTarget, expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsDeleteOutcome;
        move_file(source: &'a FsTarget, destination: &'a FsTarget, options: FsMoveOptions, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsMoveOutcome;
    }
}

struct Stack {
    kernel: Kernel,
    ctx: Context,
    fs: Arc<TrackingFs>,
    tool_fiber: Fiber,
}

async fn mount_dependencies(root: &Path) -> (Kernel, Context, Arc<TrackingFs>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fs = Arc::new(TrackingFs {
        inner: LocalFileSystem::new(LocalConfig {
            cwd: Some(path_text(root)),
            ..LocalConfig::default()
        })
        .unwrap(),
        byte_reads: AtomicUsize::new(0),
        stat_override: Mutex::new(None),
    });
    let provided = fs.clone();
    ctx.inject(move |ctx, (): ()| {
        let fs = provided.clone();
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
    ctx.plugin::<WorkingDirectory>(json!({ "defaultDirectory": path_text(root) }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx, fs)
}

async fn mount(root: &Path) -> Stack {
    let (kernel, ctx, fs) = mount_dependencies(root).await;
    let tool_fiber = ctx.plugin::<ToolPresent>(json!({ "maxFiles": 2 })).unwrap();
    tool_fiber.settled().await.unwrap();
    Stack {
        kernel,
        ctx,
        fs,
        tool_fiber,
    }
}

struct Owner {
    live: Arc<LiveAgent>,
    seam: Arc<dyn hoh_tools::Agent>,
    _scope: Scope,
    _detach: AgentDetach,
}

fn agent_in(stack: &Stack, id: &str, cwd: Option<&Path>, open_turn: bool) -> Owner {
    let key = ScopeKey::new();
    let scope = create_scope(&stack.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let id: SessionId = id.into();
    let mut header = json!({
        "version": SESSION_FORMAT_VERSION,
        "id": id,
        "createdAt": 0,
        "isSeeded": false
    });
    if let Some(cwd) = cwd {
        header["cwd"] = json!(path_text(cwd));
    }
    let session = Session::create(id.clone(), None, Some(header), None, None).unwrap();
    let live = Arc::new(LiveAgent {
        id,
        session: session.clone(),
        ctx: scope.ctx.clone(),
        scope: key.clone(),
        options: AgentOptions::default(),
    });
    let registered: Arc<dyn Agent> = live.clone();
    let detach = stack
        .ctx
        .get::<Agents>()
        .unwrap()
        .enter(&stack.ctx, registered)
        .unwrap();
    if open_turn {
        session
            .append("turn/start", json!({ "turn": 1 }), None)
            .unwrap();
    }
    Owner {
        seam: Arc::new(ToolSeam { key, session }),
        live,
        _scope: scope,
        _detach: detach,
    }
}

async fn call(
    ctx: &Context,
    files: Value,
    agent: Option<Arc<dyn hoh_tools::Agent>>,
) -> ToolExecutionResult {
    static CALL: AtomicUsize = AtomicUsize::new(1);
    let mut input = ToolExecutionInput::new(
        format!("present-call-{}", CALL.fetch_add(1, Ordering::Relaxed)).into(),
        "present",
        json!({ "files": files }),
        AbortController::new().signal(),
    );
    input.agent = agent;
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    let prepared = pipeline.prepare(input).await;
    let exec = prepared.exec().clone();
    let outcome = match prepared {
        ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        ScheduledToolPreparation::PostResult { result, .. } => {
            ScheduledToolDispatch::PostResult(result)
        }
        ScheduledToolPreparation::FinalResult { result, .. } => {
            ScheduledToolDispatch::FinalResult(result)
        }
    };
    pipeline.complete(&exec, outcome).await
}

fn deliveries(session: &Session) -> Vec<PresentedEvent> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "deliverables/presented")
        .map(|event| serde_json::from_value(event.data().clone()).unwrap())
        .collect()
}

fn error_info(result: &ToolExecutionResult) -> Option<(&str, &str)> {
    result
        .error()
        .and_then(|failure| failure.info.as_ref())
        .map(|info| (info.name.as_str(), info.code.as_str()))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn declares_binary_without_reading_or_copying_and_records_one_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-1", Some(&root), true);
    let file = root.join("报告.docx");
    std::fs::write(&file, [80, 75, 0, 255]).unwrap();

    let result = call(
        &stack.ctx,
        json!([{ "path": "报告.docx", "description": "Report" }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    let expected_files = json!([{
        "path": path_text(&std::fs::canonicalize(&file).unwrap()),
        "description": "Report"
    }]);
    assert_eq!(result.value().unwrap()["files"], expected_files);
    let recorded = deliveries(owner.live.session());
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        serde_json::to_value(&recorded[0].files).unwrap(),
        expected_files
    );
    assert_eq!(stack.fs.byte_reads.load(Ordering::SeqCst), 0);

    let tools = stack.ctx.get::<Tools>().unwrap();
    let definition = tools.get("present", Some(&owner.live.scope), None).unwrap();
    assert_eq!(
        definition.description,
        "Declare existing files as final deliverables for the user. Hosts may show presented files only after your turn ends, so call it once the files are final. If the user needs to review files earlier, such as before a tool call that waits for their answer, link them in your message instead. Use it when the user needs a separate file, especially Office documents, spreadsheets, and slide decks; prefer your final response when that suffices. The user opens the current files; their contents are not copied."
    );
    assert_eq!(result.text(), format!("Presented {}", path_text(&file)));
    stack.tool_fiber.dispose().await;
    assert!(
        tools
            .get("present", Some(&owner.live.scope), None)
            .is_none()
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn ignores_a_different_present_definition_in_the_calling_agent_scope() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-2", Some(&root), true);
    let output = DefineToolOutput {
        schema: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "turn": { "type": "integer", "required": true },
                "files": { "type": "array", "required": true, "items": { "type": "string" } }
            }
        }),
        render: Arc::new(|_, _| Vec::new()),
        presentation_meta: None,
    };
    let replacement = DefineToolOptions::new(
        "present",
        "Scoped replacement.",
        json!({}),
        output,
        |_, _| Box::pin(async { Ok(json!({ "turn": 1, "files": [] })) }),
    );
    stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .register(
            owner.live.ctx(),
            define_tool(replacement).unwrap(),
            ToolRegistrationOptions::default(),
        )
        .unwrap();

    let result = call(&stack.ctx, json!([]), Some(owner.seam.clone())).await;
    assert!(!result.is_error(), "{}", result.text());
    assert!(deliveries(owner.live.session()).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn records_once_when_ancestor_and_agent_scopes_both_mount_present() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-3", Some(&root), true);
    owner
        .live
        .ctx()
        .plugin::<ToolPresent>(json!({ "maxFiles": 2 }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    std::fs::write(root.join("a"), "a").unwrap();

    let result = call(
        &stack.ctx,
        json!([{ "path": "a" }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    let recorded = deliveries(owner.live.session());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].files[0].path, path_text(&root.join("a")));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_publish_after_post_execute_blocks_a_successful_declaration() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-4", Some(&root), true);
    std::fs::write(root.join("a"), "a").unwrap();
    stack
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, next| async move {
            next.run().await?;
            Ok(PostToolDecision::Block {
                feedback: vec![ContentBlock::text("blocked")],
                info: None,
                additional_contexts: None,
            })
        })
        .unwrap();

    let result = call(
        &stack.ctx,
        json!([{ "path": "a" }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert!(result.is_error());
    assert!(deliveries(owner.live.session()).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_missing_non_file_empty_and_excessive_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-5", Some(&root), true);
    std::fs::write(root.join("large"), "four").unwrap();
    std::os::unix::fs::symlink(std::env::temp_dir(), root.join("outside")).unwrap();
    let cases = [
        (json!([]), ("ToolArgsError", "INVALID_ARGS")),
        (json!([{ "path": "" }]), ("ToolArgsError", "INVALID_ARGS")),
        (json!([{ "path": "  " }]), ("ToolArgsError", "INVALID_ARGS")),
        (
            json!([{ "path": "\u{feff}" }]),
            ("ToolArgsError", "INVALID_ARGS"),
        ),
        (json!([{ "path": "missing" }]), ("FsError", "FS_NOT_FOUND")),
        (json!([{ "path": "." }]), ("FsError", "FS_NOT_REGULAR_FILE")),
        (
            json!([{ "path": "outside" }]),
            ("FsError", "FS_NOT_REGULAR_FILE"),
        ),
        (
            json!([{ "path": "large" }, { "path": "large" }, { "path": "large" }]),
            ("ToolArgsError", "INVALID_ARGS"),
        ),
    ];
    for (files, expected) in cases {
        let result = call(&stack.ctx, files.clone(), Some(owner.seam.clone())).await;
        assert!(result.is_error(), "{files}");
        assert_eq!(error_info(&result), Some(expected), "{files}");
    }
    assert!(deliveries(owner.live.session()).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_tool_agent_that_is_not_registered_as_a_live_agent() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let key = ScopeKey::new();
    let _scope = create_scope(&stack.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let id: SessionId = "present-unregistered".into();
    let session = Session::create(
        id.clone(),
        None,
        Some(json!({
            "version": SESSION_FORMAT_VERSION,
            "id": id,
            "createdAt": 0,
            "cwd": path_text(&root),
            "isSeeded": false
        })),
        None,
        None,
    )
    .unwrap();
    session
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    let seam: Arc<dyn hoh_tools::Agent> = Arc::new(ToolSeam {
        key,
        session: session.clone(),
    });

    let result = call(&stack.ctx, json!([{ "path": "a" }]), Some(seam)).await;

    assert_eq!(
        error_info(&result),
        Some(("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE"))
    );
    assert!(deliveries(&session).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn a_session_without_cwd_uses_the_deployment_default_before_file_validation() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-no-cwd", None, true);

    let result = call(
        &stack.ctx,
        json!([{ "path": "missing-from-default" }]),
        Some(owner.seam.clone()),
    )
    .await;

    assert_eq!(error_info(&result), Some(("FsError", "FS_NOT_FOUND")));
    assert!(deliveries(owner.live.session()).is_empty());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_plugin_config_fails_before_registering_present() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let (kernel, ctx, _) = mount_dependencies(&root).await;

    let fiber = ctx
        .plugin::<ToolPresent>(json!({ "maxFiles": 1.5 }))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "present requires a positive integer maxFiles"
    );
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("present", None, None)
            .is_none()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn requires_an_agent_and_an_open_turn() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let stack = mount(&root).await;
    let detached = call(&stack.ctx, json!([{ "path": "a" }]), None).await;
    assert_eq!(
        error_info(&detached),
        Some(("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE"))
    );

    let closed = agent_in(&stack, "present-owner-6", Some(&root), false);
    let result = call(
        &stack.ctx,
        json!([{ "path": "a" }]),
        Some(closed.seam.clone()),
    )
    .await;
    assert_eq!(
        error_info(&result),
        Some(("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE"))
    );

    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn declares_readable_files_outside_the_session_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = std::fs::canonicalize(outside.path())
        .unwrap()
        .join("report.txt");
    std::fs::write(&external, "external report").unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-8", Some(&root), true);
    let relative = pathdiff(&root, &external);

    let result = call(
        &stack.ctx,
        json!([{ "path": path_text(&external) }, { "path": relative }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    let expected = path_text(&std::fs::canonicalize(&external).unwrap());
    assert_eq!(
        deliveries(owner.live.session())[0]
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec![expected.as_str(), expected.as_str()]
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn refuses_a_final_symlink_to_an_ordinary_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("source"), "source").unwrap();
    std::os::unix::fs::symlink(root.join("source"), root.join("link")).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-9", Some(&root), true);

    let result = call(
        &stack.ctx,
        json!([{ "path": "link" }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert_eq!(
        error_info(&result),
        Some(("FsError", "FS_NOT_REGULAR_FILE"))
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn refuses_a_file_replaced_by_a_directory_after_final_component_inspection() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let source = root.join("source");
    std::fs::write(&source, "source").unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "present-owner-10", Some(&root), true);
    let root_target = stack
        .fs
        .resolve(
            &path_text(&root),
            FsResolveOptions {
                cwd: None,
                signal: None,
            },
        )
        .await
        .unwrap();
    let directory_info = stack.fs.stat(&root_target, None).await.unwrap().unwrap();
    stack.fs.override_stat(&source, directory_info);

    let result = call(
        &stack.ctx,
        json!([{ "path": "source" }]),
        Some(owner.seam.clone()),
    )
    .await;
    assert_eq!(
        error_info(&result),
        Some(("FsError", "FS_NOT_REGULAR_FILE"))
    );
    assert!(deliveries(owner.live.session()).is_empty());
    stack.kernel.shutdown().await;
}

fn pathdiff(from: &Path, to: &Path) -> String {
    let from = from.components().collect::<Vec<_>>();
    let to = to.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut path = std::path::PathBuf::new();
    for _ in common..from.len() {
        path.push("..");
    }
    for component in &to[common..] {
        path.push(component.as_os_str());
    }
    path_text(&path)
}
