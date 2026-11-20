//! Shared fixtures: the real stack the service runs on, over real temporary directories.
//! Adapted from the fixtures of `hoh-working-directory` (agent, inbox and filesystem fakes).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    Fs, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditOutcome, FsEditRequest, FsError,
    FsErrorCode, FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsPathInfo, FsPathType,
    FsResolveOptions, FsTarget, FsTextStream, FsType, FsVersion, FsVersionGuard, FsWriteIntent,
    FsWriteOutcome,
};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::UserMessage;
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause, SessionId};
use hoh_session::{CreateSessionOptions, Session, SessionStore};
use hoh_session_projection::{ProjectionHandle, SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortSignal;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tools::ToolRuntime;
use hoh_util_workspace_path::node_path;
use hoh_working_directory::{WorkingDirectory, WorkingDirectoryKey, WorkingDirectoryService};
use serde_json::json;
use tokio::sync::oneshot;

/// A parked `stat`: reports the signal it received, then waits to be released.
pub struct Park {
    pub entered: oneshot::Sender<Option<AbortSignal>>,
    pub release: oneshot::Receiver<()>,
}

/// `ctx.fs` over the real temporary directory; only the primitives the working directory uses.
#[derive(Default)]
pub struct LocalFs {
    pub park: Mutex<Option<Park>>,
    pub stats: Mutex<usize>,
}

fn unsupported<T>() -> FsFuture<'static, T> {
    Box::pin(async {
        Err(FsError::new("unsupported by the test provider", FsErrorCode::IoError).into())
    })
}

impl LocalFs {
    fn kind(meta: &std::fs::Metadata) -> FsType {
        if meta.is_dir() {
            FsType::Directory
        } else if meta.is_file() {
            FsType::File
        } else {
            FsType::Other
        }
    }
}

impl hoh_fs::FileSystem for LocalFs {
    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        Box::pin(async move {
            let cwd = match options.cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir()?.to_string_lossy().into_owned(),
            };
            let joined = node_path::resolve(&cwd, &[path]);
            let real = tokio::fs::canonicalize(&joined)
                .await
                .map(|real| real.to_string_lossy().into_owned())
                .unwrap_or(joined);
            Ok(FsTarget {
                target_key: real.clone().into(),
                display_path: real,
            })
        })
    }
    fn process_path(&self, target: &FsTarget) -> String {
        target.display_path.clone()
    }
    fn file_url(&self, target: &FsTarget) -> String {
        format!("file://{}", target.display_path)
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        child.display_path == parent.display_path
            || child
                .display_path
                .starts_with(&format!("{}/", parent.display_path))
    }
    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            *self.stats.lock().unwrap() += 1;
            let park = self.park.lock().unwrap().take();
            let info = match tokio::fs::metadata(&target.display_path).await {
                Ok(meta) => Some(FsInfo {
                    version: FsVersion::from(format!("{}", meta.len())),
                    r#type: Self::kind(&meta),
                    size: Some(meta.len()),
                }),
                Err(_) => None,
            };
            if let Some(park) = park {
                let _ = park.entered.send(signal);
                let _ = park.release.await;
            }
            Ok(info)
        })
    }
    fn lstat<'a>(
        &'a self,
        path: &'a str,
        _cwd: Option<&'a str>,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        Box::pin(async move {
            Ok(tokio::fs::symlink_metadata(path)
                .await
                .ok()
                .map(|meta| FsPathInfo {
                    version: FsVersion::from(format!("{}", meta.len())),
                    r#type: FsPathType::from(Self::kind(&meta)),
                    size: Some(meta.len()),
                }))
        })
    }
    fn read_text<'a>(&'a self, _t: &'a FsTarget, _s: Option<AbortSignal>) -> FsFuture<'a, String> {
        unsupported()
    }
    fn stream_text<'a>(
        &'a self,
        _t: &'a FsTarget,
        _s: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        unsupported()
    }
    fn read_bytes<'a>(
        &'a self,
        _t: &'a FsTarget,
        _s: Option<AbortSignal>,
        _m: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        unsupported()
    }
    fn read_byte_range<'a>(
        &'a self,
        _t: &'a FsTarget,
        _r: FsByteRange,
        _s: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        unsupported()
    }
    fn list_dir<'a>(
        &'a self,
        _t: &'a FsTarget,
        _s: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        unsupported()
    }
    fn write_text<'a>(
        &'a self,
        _t: &'a FsTarget,
        _c: &'a str,
        _e: Option<FsWriteIntent>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        unsupported()
    }
    fn edit_text<'a>(
        &'a self,
        _t: &'a FsTarget,
        _e: FsEditRequest,
        _g: Option<FsVersionGuard>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        unsupported()
    }
    fn patch_text<'a>(
        &'a self,
        _t: &'a FsTarget,
        _h: &'a [hoh_fs_patch::PatchHunk],
        _g: Option<FsVersionGuard>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        unsupported()
    }
    fn delete_file<'a>(
        &'a self,
        _t: &'a FsTarget,
        _g: Option<FsVersionGuard>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        unsupported()
    }
    fn move_file<'a>(
        &'a self,
        _s: &'a FsTarget,
        _d: &'a FsTarget,
        _o: FsMoveOptions,
        _sig: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        unsupported()
    }
}

struct UnusedModel;
impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture does not drive the Agent".into())
    }
}

/// A `hoh_agent::Agent` over a real session whose inbox is the driver's `ReactLoopInbox`
/// (`agent.inject` queues next-step input like the real agent). Only the parts the working
/// directory touches are real; the driver and model are absent.
pub struct FakeAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: ReactLoopInbox,
    pub fail_inject: AtomicBool,
}

impl Agent for FakeAgent {
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
        Ok(task(hoh_timeout::AbortController::new().signal()))
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
        if self.fail_inject.load(Ordering::SeqCst) {
            return Err("inbox unavailable".into());
        }
        self.inbox.append(InboxTarget::NextStep, message)
    }
}

/// The agent handle plus what a test needs to reach behind it.
#[derive(Clone)]
pub struct TestAgent {
    pub agent: Arc<dyn Agent>,
    pub fake: Arc<FakeAgent>,
    pub fiber: Fiber,
}

impl TestAgent {
    pub fn session(&self) -> Session {
        self.fake.session.clone()
    }
    /// `harness.claim(agent, 'next-step', n)`: the notices queued so far, removed from the inbox.
    pub fn claim(&self) -> Vec<UserMessage> {
        let queued = self.fake.inbox.try_next_step().expect("inbox state");
        self.fake.inbox.clear().expect("clear inbox");
        queued
    }
}

pub struct Fixture {
    pub kernel: Kernel,
    pub ctx: Context,
    pub store: SessionStore,
    pub inbox: Arc<ProjectionHandle<InboxState>>,
    pub service: WorkingDirectoryService,
    pub dirs: Mutex<Vec<tempfile::TempDir>>,
}

impl Fixture {
    /// Session store, projections, agent registry, system prompt, filesystem and working
    /// directory. The tool registry is mounted on demand with [`Fixture::mount_tools`].
    pub async fn new() -> Self {
        Self::with_default_directory(None).await
    }

    pub async fn with_default_directory(default_directory: Option<&str>) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let fs = Arc::new(LocalFs::default());
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
        let config = match default_directory {
            Some(directory) => json!({ "defaultDirectory": directory }),
            None => json!({}),
        };
        ctx.plugin::<WorkingDirectory>(config)
            .unwrap()
            .settled()
            .await
            .unwrap();
        let service = (*ctx
            .get::<WorkingDirectoryKey>()
            .expect("service is provided"))
        .clone();
        let store = SessionStore::new();
        let inbox = Arc::new(
            ctx.get::<SessionProjections>()
                .expect("sessionProjections")
                .register(&ctx, inbox_projection_definition())
                .await
                .expect("inbox projection"),
        );
        Self {
            kernel,
            ctx,
            store,
            inbox,
            service,
            dirs: Mutex::new(Vec::new()),
        }
    }

    pub async fn mount_tools(&self) {
        self.ctx
            .plugin::<ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        self.ctx
            .plugin::<NativeToolExecutorPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
    }

    /// A temporary directory holding `README.md`, kept alive with the fixture.
    pub fn workspace(&self, prefix: &str, files: &[&str]) -> PathBuf {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for name in files {
            std::fs::write(root.join(name), name).unwrap();
        }
        self.dirs.lock().unwrap().push(dir);
        root
    }

    /// An agent whose Session records `cwd` as its project (`None`: no recorded directory),
    /// registered with `agent/created` announced, as `ctx.agents.register` does.
    pub async fn register_agent(&self, id: &str, cwd: Option<&str>) -> TestAgent {
        let (agent, disposer) = self.register_disposable_agent(id, cwd).await;
        std::mem::forget(disposer);
        agent
    }

    /// Like [`Fixture::register_agent`], but the caller removes the agent from the registry.
    pub async fn register_disposable_agent(
        &self,
        id: &str,
        cwd: Option<&str>,
    ) -> (TestAgent, hoh_kernel::Disposer) {
        let agent = self.make_agent(id, cwd).await;
        let registry = self.ctx.get::<Agents>().expect("agents");
        let owner = agent.agent.ctx().clone();
        let disposer = registry
            .register(&owner, agent.agent.clone())
            .await
            .expect("agent registered");
        (agent, disposer)
    }

    /// An agent that is not announced to the registry.
    pub async fn make_agent(&self, id: &str, cwd: Option<&str>) -> TestAgent {
        make_agent(&self.ctx, &self.store, &self.inbox, id, cwd).await
    }

    pub async fn shutdown(self) {
        self.kernel.shutdown().await;
    }
}

fn agent_over(
    session: Session,
    ctx: Context,
    fiber: Fiber,
    inbox: &Arc<ProjectionHandle<InboxState>>,
    scope: ScopeKey,
) -> TestAgent {
    let (read, reader_session) = (inbox.clone(), session.clone());
    let fake = Arc::new_cyclic(|weak: &std::sync::Weak<FakeAgent>| {
        let weak = weak.clone();
        let dispatch_ctx = ctx.clone();
        FakeAgent {
            id: session.id().clone(),
            inbox: ReactLoopInbox::new(
                Arc::new(move || read.state_of(&reader_session)),
                session.clone(),
                Arc::new(move || {
                    let agent: Arc<dyn Agent> = weak.upgrade()?;
                    Some(agent_events(&dispatch_ctx, &agent))
                }),
            ),
            session,
            ctx,
            scope,
            options: AgentOptions::default(),
            model: UnusedModel,
            fail_inject: AtomicBool::new(false),
        }
    });
    TestAgent {
        agent: fake.clone(),
        fake,
        fiber,
    }
}

/// A Session in `store` owned by an Agent whose context is its own scope of `ctx`.
pub async fn make_agent(
    ctx: &Context,
    store: &SessionStore,
    inbox: &Arc<ProjectionHandle<InboxState>>,
    id: &str,
    cwd: Option<&str>,
) -> TestAgent {
    let key = ScopeKey::new();
    let scope = create_scope(ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let agent_ctx = scope.ctx.clone();
    let fiber = agent_ctx.fiber();
    let meta = match cwd {
        Some(cwd) => json!({ "cwd": cwd }),
        None => json!({}),
    };
    let session = store
        .create(
            ctx,
            Some(id.into()),
            CreateSessionOptions {
                meta: Some(meta),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    agent_over(session, agent_ctx, fiber, inbox, key)
}

pub fn texts(messages: &[UserMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| block.text_value().map(str::to_owned))
        .collect()
}
