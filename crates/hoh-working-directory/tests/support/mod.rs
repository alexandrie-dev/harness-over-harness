//! Shared fixtures: the real stack of the package under test over a real temporary directory.
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
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, SessionId};
use hoh_session::{CreateSessionOptions, Session, SessionStore};
use hoh_session_projection::{ProjectionHandle, SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortSignal;
use hoh_util_workspace_path::node_path;
use hoh_working_directory::{WorkingDirectory, WorkingDirectoryKey, WorkingDirectoryService};
use serde_json::{Value, json};
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

    fn version(meta: &std::fs::Metadata) -> FsVersion {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let mtime = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
            let ctime = i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec());
            format!(
                "{}:{}:{}:{mtime}:{ctime}",
                meta.dev(),
                meta.ino(),
                meta.len()
            )
            .into()
        }
        #[cfg(not(unix))]
        {
            meta.len().to_string().into()
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
                    version: Self::version(&meta),
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
                    version: Self::version(&meta),
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
    pub root: PathBuf,
    pub root_text: String,
    pub dir: tempfile::TempDir,
    pub store: SessionStore,
    pub inbox: Arc<ProjectionHandle<InboxState>>,
    pub fs: Arc<LocalFs>,
    pub directory_fiber: Fiber,
    pub service: WorkingDirectoryService,
    pub agent: TestAgent,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_config(|_| json!({})).await
    }

    /// `fixture(config)`: mounts the dependencies and the plugin; a failing plugin start is returned.
    pub async fn with_config(config: impl FnOnce(&str) -> Value) -> Self {
        Self::try_with_config(config).await.expect("fixture")
    }

    pub async fn try_with_config(
        config: impl FnOnce(&str) -> Value,
    ) -> Result<Self, hoh_kernel::KernelError> {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let root_text = root.to_string_lossy().into_owned();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let fs = Arc::new(LocalFs::default());
        let provided = fs.clone();
        ctx.inject(move |ctx, (): ()| {
            let fs = provided.clone();
            async move {
                ctx.provide::<Fs>(fs).await?;
                Ok(())
            }
        })?
        .settled()
        .await?;
        ctx.plugin::<SessionProjectionRegistry>(json!({}))?
            .settled()
            .await?;
        ctx.plugin::<AgentRegistryPlugin>(json!({}))?
            .settled()
            .await?;
        ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))?
            .settled()
            .await?;
        let directory_fiber = ctx.plugin::<WorkingDirectory>(config(&root_text))?;
        if let Err(error) = directory_fiber.settled().await {
            kernel.shutdown().await;
            return Err(error);
        }
        let service = (*ctx
            .get::<WorkingDirectoryKey>()
            .expect("service is provided"))
        .clone();
        let store = SessionStore::new();
        let inbox_projection = Arc::new(
            ctx.get::<SessionProjections>()
                .expect("sessionProjections")
                .register(&ctx, inbox_projection_definition())
                .await
                .expect("inbox projection"),
        );
        let agent = make_agent(
            &ctx,
            &store,
            &inbox_projection,
            "directory-owner",
            Some(&root_text),
        )
        .await;
        let fixture = Self {
            kernel,
            ctx,
            root,
            root_text,
            dir,
            store,
            inbox: inbox_projection,
            fs,
            directory_fiber,
            service,
            agent,
        };
        Ok(fixture)
    }

    /// `harness.create(id, {}, { cwd })`.
    pub async fn create(&self, id: &str, cwd: Option<&str>) -> TestAgent {
        make_agent(&self.ctx, &self.store, &self.inbox, id, cwd).await
    }

    /// An Agent over an existing Session (restored or created elsewhere).
    pub fn agent_for(&self, session: Session, ctx: Context, fiber: Fiber) -> TestAgent {
        agent_over(session, ctx, fiber, &self.inbox)
    }

    pub fn path(&self, parts: &[&str]) -> String {
        let mut path = self.root.clone();
        for part in parts {
            path.push(part);
        }
        path.to_string_lossy().into_owned()
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
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: UnusedModel,
            fail_inject: AtomicBool::new(false),
        }
    });
    // The calling composition finds the whole Agent in the registry by session id.
    if let Some(registry) = fake.ctx.get::<Agents>()
        && registry.get(fake.session.id()).is_none()
    {
        let registered: Arc<dyn Agent> = fake.clone();
        std::mem::forget(
            registry
                .enter(&fake.ctx, registered)
                .expect("agent registered"),
        );
    }
    TestAgent {
        agent: fake.clone(),
        fake,
        fiber,
    }
}

/// A Session in `store` owned by an Agent whose context is its own fiber of `ctx`.
pub async fn make_agent(
    ctx: &Context,
    store: &SessionStore,
    inbox: &Arc<ProjectionHandle<InboxState>>,
    id: &str,
    cwd: Option<&str>,
) -> TestAgent {
    let slot = Arc::new(Mutex::new(None));
    let taken = slot.clone();
    let fiber = ctx
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
    agent_over(session, agent_ctx, fiber, inbox)
}

pub fn texts(messages: &[UserMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| block.text_value().map(str::to_owned))
        .collect()
}
