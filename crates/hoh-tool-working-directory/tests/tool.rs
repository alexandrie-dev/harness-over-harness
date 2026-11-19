//! The `working_directory` tool over the real working-directory service, real agents (registered in
//! the `agents` registry, driving a real `ReactLoopInbox`), the real session-projection registry and a
//! real directory.

use std::path::Path;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxState, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
    agent_events,
};
use hoh_agent_loop::{ReactLoopInbox, inbox_projection_definition};
use hoh_error::BoxError;
use hoh_fs::{
    FileSystem, Fs, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditOutcome, FsEditRequest,
    FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsPathInfo, FsResolveOptions, FsTarget,
    FsTextStream, FsVersionGuard, FsWriteIntent, FsWriteOutcome,
};
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::UserMessage;
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionStore};
use hoh_session_projection::{ProjectionHandle, SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_working_directory::ToolWorkingDirectory;
use hoh_tools::{
    ScheduledToolDispatch, ScheduledToolPreparation, ToolExecutionInput, ToolExecutionPipeline,
    ToolExecutionResult, ToolRuntime, Tools,
};
use hoh_working_directory::WorkingDirectory;
use serde_json::{Value, json};
use tokio::sync::oneshot;

struct UnusedModel;
impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture does not drive the Agent".into())
    }
}

/// A `hoh_agent::Agent` over a real session; `inject` queues next-step input in the real inbox.
struct LiveAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: ReactLoopInbox,
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
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
}

/// A parked `stat`: the test is told the call arrived and decides when it returns.
struct Park {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// The real local filesystem with one parkable `stat` (the window between the directory check and
/// the commit).
struct ParkFs {
    inner: LocalFileSystem,
    park: Mutex<Option<Park>>,
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(fn $name<'a>(&'a self, $($arg: $ty),*) -> FsFuture<'a, $ret> {
            self.inner.$name($($arg),*)
        })*
    };
}

impl FileSystem for ParkFs {
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
            let info = self.inner.stat(target, signal).await?;
            let park = self.park.lock().unwrap().take();
            if let Some(park) = park {
                let _ = park.entered.send(());
                let _ = park.release.await;
            }
            Ok(info)
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
    delegate! {
        read_text(target: &'a FsTarget, signal: Option<AbortSignal>) -> String;
        stream_text(target: &'a FsTarget, signal: Option<AbortSignal>) -> FsTextStream;
        read_bytes(target: &'a FsTarget, signal: Option<AbortSignal>, max: u64) -> Vec<u8>;
        read_byte_range(target: &'a FsTarget, range: FsByteRange, signal: Option<AbortSignal>) -> Vec<u8>;
        list_dir(target: &'a FsTarget, signal: Option<AbortSignal>) -> Vec<FsDirEntry>;
        write_text(target: &'a FsTarget, content: &'a str, expect: Option<FsWriteIntent>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsWriteOutcome;
        edit_text(target: &'a FsTarget, edit: FsEditRequest, guard: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsEditOutcome;
        patch_text(target: &'a FsTarget, hunks: &'a [hoh_fs_patch::PatchHunk], guard: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsEditOutcome;
        delete_file(target: &'a FsTarget, guard: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsDeleteOutcome;
        move_file(source: &'a FsTarget, destination: &'a FsTarget, options: FsMoveOptions, signal: Option<AbortSignal>, policy: Option<SandboxExecutionPolicy>) -> FsMoveOutcome;
    }
}

struct Stack {
    kernel: Kernel,
    ctx: Context,
    fs: Arc<ParkFs>,
    store: SessionStore,
    inbox: Arc<ProjectionHandle<InboxState>>,
    tool_fiber: Fiber,
}

async fn mount(root: &Path) -> Stack {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fs = Arc::new(ParkFs {
        inner: LocalFileSystem::new(LocalConfig {
            cwd: Some(path_text(root)),
            ..LocalConfig::default()
        })
        .unwrap(),
        park: Mutex::new(None),
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
    ctx.plugin::<WorkingDirectory>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let tool_fiber = ctx.plugin::<ToolWorkingDirectory>(json!(null)).unwrap();
    tool_fiber.settled().await.unwrap();
    let inbox = Arc::new(
        ctx.get::<SessionProjections>()
            .unwrap()
            .register(&ctx, inbox_projection_definition())
            .await
            .unwrap(),
    );
    Stack {
        kernel,
        ctx,
        fs,
        store: SessionStore::new(),
        inbox,
        tool_fiber,
    }
}

/// One agent: its own fiber (disposing it ends the agent's context), a real session in `cwd`, a
/// real inbox, registered in the `agents` registry.
struct Owner {
    live: Arc<LiveAgent>,
    seam: Arc<dyn hoh_tools::Agent>,
    fiber: Fiber,
}

impl Owner {
    /// The notices queued so far in the real inbox.
    fn queued(&self) -> Vec<String> {
        self.live
            .inbox
            .try_next_step()
            .unwrap()
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| block.text_value().map(str::to_owned))
            .collect()
    }
}

async fn agent_in(stack: &Stack, id: &str, cwd: &Path) -> Owner {
    let slot = Arc::new(Mutex::new(None));
    let taken = slot.clone();
    let fiber = stack
        .ctx
        .inject(move |ctx, (): ()| {
            let slot = taken.clone();
            async move {
                *slot.lock().unwrap() = Some(ctx);
                Ok(())
            }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let agent_ctx = slot.lock().unwrap().take().unwrap();
    let session = stack
        .store
        .create(
            &stack.ctx,
            Some(id.into()),
            CreateSessionOptions {
                meta: Some(json!({ "cwd": path_text(cwd) })),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    let (read, reader_session) = (stack.inbox.clone(), session.clone());
    let live = Arc::new_cyclic(|weak: &std::sync::Weak<LiveAgent>| {
        let weak = weak.clone();
        let dispatch_ctx = stack.ctx.clone();
        LiveAgent {
            id: session.id().clone(),
            inbox: ReactLoopInbox::new(
                Arc::new(move || read.state_of(&reader_session)),
                session.clone(),
                Arc::new(move || {
                    let agent: Arc<dyn Agent> = weak.upgrade()?;
                    Some(agent_events(&dispatch_ctx, &agent))
                }),
            ),
            session: session.clone(),
            ctx: agent_ctx,
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: UnusedModel,
        }
    });
    let registry = stack.ctx.get::<Agents>().unwrap();
    let registered: Arc<dyn Agent> = live.clone();
    // Kept registered for the stack's lifetime; the registry unwinds with the kernel.
    std::mem::forget(registry.enter(&stack.ctx, registered).unwrap());
    Owner {
        seam: live.clone(),
        live,
        fiber,
    }
}

async fn call(
    ctx: &Context,
    arguments: Value,
    agent: Option<Arc<dyn hoh_tools::Agent>>,
) -> ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        "directory-call".into(),
        "working_directory",
        arguments,
        AbortController::new().signal(),
    );
    input.agent = agent;
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).expect("pipeline");
    let prepared = pipeline
        .prepare(input)
        .await
        .expect("the tool lookup does not reject");
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

fn error_info(result: &ToolExecutionResult) -> Option<(String, String)> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| (info.name.clone(), info.code.clone()))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn exposes_reading_and_changing_through_the_same_model_tool_and_unregisters_it_on_disposal() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "directory-owner", &root).await;
    let ctx = &stack.ctx;
    let agent = owner.seam.clone();

    let read = call(ctx, json!({}), Some(agent.clone())).await;
    assert!(!read.is_error(), "{}", read.text());
    assert_eq!(read.value(), Some(&json!({ "cwd": path_text(&root) })));
    assert_eq!(read.text(), path_text(&root));

    let changed = call(ctx, json!({ "cd": "child" }), Some(agent.clone())).await;
    assert!(!changed.is_error(), "{}", changed.text());
    assert_eq!(
        changed.value(),
        Some(&json!({ "cwd": path_text(&root.join("child")) }))
    );

    let no_agent = call(ctx, json!({}), None).await;
    assert!(no_agent.is_error());
    assert_eq!(
        error_info(&no_agent),
        Some((
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        ))
    );

    let empty = call(ctx, json!({ "cd": "" }), Some(agent.clone())).await;
    assert!(empty.is_error());
    assert_eq!(
        error_info(&empty),
        Some(("ToolArgsError".into(), "INVALID_ARGS".into()))
    );

    let missing = call(ctx, json!({ "cd": "missing" }), Some(agent)).await;
    assert!(missing.is_error());
    assert_eq!(
        error_info(&missing),
        Some((
            "WorkingDirectoryError".into(),
            "WORKING_DIRECTORY_NOT_FOUND".into()
        ))
    );

    stack.tool_fiber.dispose().await;
    let tools = ctx.get::<Tools>().unwrap();
    assert!(
        !tools
            .schemas(None, None)
            .unwrap()
            .iter()
            .any(|tool| tool.name == "working_directory")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn queues_the_directory_change_notice_in_the_real_agent_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "notice-owner", &root).await;

    let read = call(&stack.ctx, json!({}), Some(owner.seam.clone())).await;
    assert!(!read.is_error(), "{}", read.text());
    assert!(owner.queued().is_empty(), "reading changes nothing");

    let child = path_text(&root.join("child"));
    let changed = call(
        &stack.ctx,
        json!({ "cd": "child" }),
        Some(owner.seam.clone()),
    )
    .await;
    assert!(!changed.is_error(), "{}", changed.text());
    assert_eq!(
        owner.queued(),
        [format!(
            "The working directory changed from \"{}\" to \"{child}\".",
            path_text(&root)
        )]
    );
    let events = owner.live.session.snapshot_events(None, None);
    let changes: Vec<_> = events
        .iter()
        .filter(|event| event.event_type() == "working-directory/change")
        .map(|event| event.data()["cwd"].clone())
        .collect();
    assert_eq!(changes, [json!(child)]);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn commits_nothing_when_the_agent_is_disposed_between_the_directory_check_and_the_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let stack = mount(&root).await;
    let owner = agent_in(&stack, "disposed-owner", &root).await;
    // Record the original directory first so the later session log holds only the change under test.
    let first = call(&stack.ctx, json!({}), Some(owner.seam.clone())).await;
    assert!(!first.is_error(), "{}", first.text());
    let events_before = owner.live.session.snapshot_events(None, None).len();

    let (entered, arrived) = oneshot::channel();
    let (release, released) = oneshot::channel();
    *stack.fs.park.lock().unwrap() = Some(Park {
        entered,
        release: released,
    });
    let pending = tokio::spawn({
        let ctx = stack.ctx.clone();
        let seam = owner.seam.clone();
        async move { call(&ctx, json!({ "cd": "child" }), Some(seam)).await }
    });
    arrived.await.expect("the directory check is in flight");
    owner.fiber.dispose().await;
    let _ = release.send(());
    let result = pending.await.unwrap();

    assert!(
        result.is_error(),
        "a disposed agent cannot change directory"
    );
    assert_eq!(
        owner.live.session.snapshot_events(None, None).len(),
        events_before,
        "nothing was committed after the disposal"
    );
    assert!(owner.queued().is_empty(), "no notice either");
    stack.kernel.shutdown().await;
}
