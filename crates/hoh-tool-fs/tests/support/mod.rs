//! Shared fixtures: an in-memory fake provider (`FakeFs`), the real observation policy, and the
//! real tool stack of the package under test (system prompt, tool runtime, native executor,
//! working directory) in a fresh kernel.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::stream;
use hoh_agent::{
    AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox, InboxTarget,
    Live, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    Fs, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditOutcome, FsEditRequest, FsError,
    FsErrorCode, FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsPathInfo, FsPathType,
    FsResolveOptions, FsTarget, FsTextStream, FsType, FsVersion, FsVersionGuard, FsWriteIntent,
    FsWriteOperation, FsWriteOutcome,
};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::ToolCallId;
use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_session::{AgentCancelCause, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_fs::ToolFs;
use hoh_tools::{
    Agent, ScheduledToolDispatch, ScheduledToolPreparation, ToolExecutionInput,
    ToolExecutionPipeline, ToolExecutionResult, ToolRuntime, Tools,
};
use hoh_user_approval::{ApprovalOutcome, ApprovalRequestEvent, ApprovalService};
use hoh_working_directory::WorkingDirectory;
use serde_json::{Value, json};

pub mod patch;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The directory every agent's Session falls back to.
pub const DEFAULT_DIRECTORY: &str = "/work";

/// Mutable state of the in-memory provider.
#[derive(Default)]
pub struct FakeState {
    pub files: HashMap<String, String>,
    pub directories: HashSet<String>,
    /// Arm every `stat`, `write_text` and `edit_text` with this rejection.
    pub reject_with: Option<(String, FsErrorCode)>,
    pub write_intents: Vec<Option<FsWriteIntent>>,
    pub edit_intents: Vec<Option<FsVersionGuard>>,
    /// Report no size on `stat` (the backend cannot tell it).
    pub hide_size: bool,
    /// Report a fixed size on `stat` regardless of the content.
    pub fixed_size: Option<u64>,
    /// Report this type for `stat` of any file.
    pub type_override: Option<FsType>,
    pub bytes: HashMap<String, Vec<u8>>,
    /// Label targets with this scheme instead of `/abs/` (a provider whose display differs).
    pub display_prefix: Option<String>,
    /// Answer `process_path` with this path regardless of the target.
    pub process_path: Option<String>,
    /// The provider confines: it reports this default sandbox mode.
    pub confining: Option<SandboxMode>,
    /// The policy stamped onto each mutation, in call order.
    pub stamped: Vec<Option<SandboxExecutionPolicy>>,
}

/// An in-memory fake provider; a test can arm a rejection on any primitive.
#[derive(Default)]
pub struct FakeFs {
    pub state: Mutex<FakeState>,
    pub read_text_calls: AtomicUsize,
    pub stream_text_calls: AtomicUsize,
}

impl FakeFs {
    pub fn set(&self, path: &str, content: &str) {
        lock(&self.state)
            .files
            .insert(format!("key:{path}"), content.to_owned());
    }
    pub fn get(&self, path: &str) -> Option<String> {
        lock(&self.state).files.get(&format!("key:{path}")).cloned()
    }
    pub fn set_bytes(&self, path: &str, bytes: Vec<u8>) {
        lock(&self.state).bytes.insert(format!("key:{path}"), bytes);
    }
    pub fn reject_with(&self, message: &str, code: FsErrorCode) {
        lock(&self.state).reject_with = Some((message.to_owned(), code));
    }
    pub fn write_intents(&self) -> Vec<Option<FsWriteIntent>> {
        lock(&self.state).write_intents.clone()
    }
    pub fn edit_intents(&self) -> Vec<Option<FsVersionGuard>> {
        lock(&self.state).edit_intents.clone()
    }

    fn throw_if_armed(&self) -> Result<(), BoxError> {
        match &lock(&self.state).reject_with {
            Some((message, code)) => Err(FsError::new(message.clone(), *code).into()),
            None => Ok(()),
        }
    }
}

fn unsupported<T>(message: &'static str) -> FsFuture<'static, T> {
    Box::pin(async move { Err(FsError::new(message, FsErrorCode::IoError).into()) })
}

impl hoh_fs::FileSystem for FakeFs {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        lock(&self.state).confining
    }
    fn resolve<'a>(&'a self, path: &'a str, _options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        Box::pin(async move {
            let prefix = lock(&self.state)
                .display_prefix
                .clone()
                .unwrap_or_else(|| "/abs/".to_owned());
            Ok(FsTarget {
                target_key: format!("key:{path}").into(),
                display_path: format!("{prefix}{path}"),
            })
        })
    }
    fn process_path(&self, target: &FsTarget) -> String {
        lock(&self.state)
            .process_path
            .clone()
            .unwrap_or_else(|| target.display_path.clone())
    }
    fn file_url(&self, target: &FsTarget) -> String {
        format!("file://{}", target.target_key.as_ref() as &str)
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        let (parent, child): (&str, &str) = (parent.target_key.as_ref(), child.target_key.as_ref());
        child == parent || child.starts_with(&format!("{parent}/"))
    }
    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            let state = lock(&self.state);
            let key: &str = target.target_key.as_ref();
            // The working directory (a real service here, a fixture in the source) validates its
            // directory with a stat that an armed rejection must not hit.
            if state.directories.contains(key) {
                return Ok(Some(FsInfo {
                    version: FsVersion::from("dir"),
                    r#type: FsType::Directory,
                    size: None,
                }));
            }
            drop(state);
            self.throw_if_armed()?;
            let state = lock(&self.state);
            let Some(content) = state.files.get(key) else {
                if let Some(bytes) = state.bytes.get(key) {
                    return Ok(Some(FsInfo {
                        version: FsVersion::from("v1"),
                        r#type: FsType::File,
                        size: Some(bytes.len() as u64),
                    }));
                }
                return Ok(None);
            };
            let size = if state.hide_size {
                None
            } else {
                Some(state.fixed_size.unwrap_or(content.len() as u64))
            };
            Ok(Some(FsInfo {
                version: FsVersion::from("v1"),
                r#type: state.type_override.unwrap_or(FsType::File),
                size,
            }))
        })
    }
    fn lstat<'a>(
        &'a self,
        path: &'a str,
        _cwd: Option<&'a str>,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        Box::pin(async move {
            let state = lock(&self.state);
            Ok(state
                .files
                .get(&format!("key:{path}"))
                .map(|content| FsPathInfo {
                    version: FsVersion::from("v1"),
                    r#type: FsPathType::File,
                    size: Some(content.len() as u64),
                }))
        })
    }
    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        Box::pin(async move {
            self.read_text_calls.fetch_add(1, Ordering::SeqCst);
            Ok(lock(&self.state)
                .files
                .get(target.target_key.as_ref() as &str)
                .cloned()
                .unwrap_or_default())
        })
    }
    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        Box::pin(async move {
            self.stream_text_calls.fetch_add(1, Ordering::SeqCst);
            let content = lock(&self.state)
                .files
                .get(target.target_key.as_ref() as &str)
                .cloned()
                .unwrap_or_default();
            let chunks: FsTextStream = Box::pin(stream::iter([Ok(content)]));
            Ok(chunks)
        })
    }
    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        _signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let state = lock(&self.state);
            let key: &str = target.target_key.as_ref();
            let bytes = match state.bytes.get(key) {
                Some(bytes) => bytes.clone(),
                None => state
                    .files
                    .get(key)
                    .cloned()
                    .unwrap_or_default()
                    .into_bytes(),
            };
            if bytes.len() as u64 > max_bytes {
                return Err(FsError::new(
                    format!("too large: {}", target.display_path),
                    FsErrorCode::TooLarge,
                )
                .into());
            }
            Ok(bytes)
        })
    }
    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let content = lock(&self.state)
                .files
                .get(target.target_key.as_ref() as &str)
                .cloned()
                .unwrap_or_default();
            let bytes = content.into_bytes();
            let start = (range.offset as usize).min(bytes.len());
            let end = ((range.offset + range.length) as usize).min(bytes.len());
            Ok(bytes[start..end].to_vec())
        })
    }
    fn list_dir<'a>(
        &'a self,
        _target: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn write_text<'a>(
        &'a self,
        target: &'a FsTarget,
        content: &'a str,
        expected: Option<FsWriteIntent>,
        _signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        Box::pin(async move {
            lock(&self.state).stamped.push(policy);
            self.throw_if_armed()?;
            let mut state = lock(&self.state);
            state.write_intents.push(expected);
            let key = target.target_key.as_ref() as &str;
            let before = state.files.get(key).cloned();
            state.files.insert(key.to_owned(), content.to_owned());
            Ok(FsWriteOutcome {
                operation: if before.is_some() {
                    FsWriteOperation::Update
                } else {
                    FsWriteOperation::Create
                },
                version: FsVersion::from("v2"),
                before,
                after: content.to_owned(),
            })
        })
    }
    fn edit_text<'a>(
        &'a self,
        target: &'a FsTarget,
        edit: FsEditRequest,
        expected: Option<FsVersionGuard>,
        _signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        Box::pin(async move {
            lock(&self.state).stamped.push(policy);
            self.throw_if_armed()?;
            let mut state = lock(&self.state);
            state.edit_intents.push(expected);
            let key = target.target_key.as_ref() as &str;
            let content = state.files.get(key).cloned().unwrap_or_default();
            let after = content
                .split(edit.old_string.as_str())
                .collect::<Vec<_>>()
                .join(&edit.new_string);
            state.files.insert(key.to_owned(), after.clone());
            Ok(FsEditOutcome {
                version: FsVersion::from("v3"),
                before: content,
                after,
            })
        })
    }
    fn patch_text<'a>(
        &'a self,
        _target: &'a FsTarget,
        _hunks: &'a [hoh_fs_patch::PatchHunk],
        _expected: Option<FsVersionGuard>,
        _signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        unsupported("patch unsupported by test provider")
    }
    fn delete_file<'a>(
        &'a self,
        _target: &'a FsTarget,
        _expected: Option<FsVersionGuard>,
        _signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        unsupported("delete unsupported by test provider")
    }
    fn move_file<'a>(
        &'a self,
        _source: &'a FsTarget,
        _destination: &'a FsTarget,
        _options: FsMoveOptions,
        _signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        unsupported("move unsupported by test provider")
    }
}

/// An agent stand-in: a scope key and a standalone session (the owner the policy keys on).
pub struct TestAgent {
    pub key: ScopeKey,
    pub session: Session,
    options: AgentOptions,
}

impl TestAgent {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            key: ScopeKey::new(),
            session: Session::create("test".into(), None, None, None, None).expect("session"),
            options: AgentOptions::default(),
        })
    }
}

impl TestAgent {
    /// An agent whose Session was created in `cwd` (the session's original project directory).
    pub fn in_dir(ctx: &Context, id: &str, cwd: &str) -> Arc<Self> {
        let session = hoh_session::SessionStore::new()
            .create(
                ctx,
                Some(id.into()),
                hoh_session::CreateSessionOptions {
                    meta: Some(json!({ "cwd": cwd })),
                    ..hoh_session::CreateSessionOptions::default()
                },
            )
            .expect("session");
        Arc::new(Self {
            key: ScopeKey::new(),
            session,
            options: AgentOptions::default(),
        })
    }
}

/// `agent.options.provider` / `.model` of the registered agent of a Session (none by default).
static ROUTES: Mutex<std::collections::BTreeMap<String, (String, String)>> =
    Mutex::new(std::collections::BTreeMap::new());

impl TestAgent {
    /// Give the agent behind this stand-in the route `agent.options` carries.
    pub fn set_route(&self, provider: &str, model: &str) {
        lock(&ROUTES).insert(
            self.session.id().to_string(),
            (provider.to_owned(), model.to_owned()),
        );
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

struct UnsupportedInbox;
impl Inbox for UnsupportedInbox {
    fn next_turn(&self) -> Vec<hoh_llm::UserMessage> {
        vec![]
    }
    fn next_step(&self) -> Vec<hoh_llm::UserMessage> {
        vec![]
    }
    fn clear(&self) -> Result<(), BoxError> {
        Err("this test Agent has no inbox".into())
    }
    fn append(&self, _: InboxTarget, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        self.clear()
    }
    fn prepend(&self, _: InboxTarget, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        self.clear()
    }
    fn replace(&self, _: &hoh_llm::MessageId, _: hoh_llm::UserMessage) -> Result<bool, BoxError> {
        self.clear().map(|()| false)
    }
    fn remove(&self, _: &hoh_llm::MessageId) -> Result<bool, BoxError> {
        self.clear().map(|()| false)
    }
    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<hoh_llm::UserMessage>,
    ) -> Result<Vec<hoh_llm::UserMessage>, BoxError> {
        self.clear().map(|()| vec![])
    }
}

/// The registered `hoh_agent::Agent` behind a [`TestAgent`]: the same Session, the route the test
/// set, and `inject` recording the notices it is given.
pub struct LiveAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: UnsupportedInbox,
    pub injected: Mutex<Vec<hoh_llm::UserMessage>>,
}

impl hoh_agent::Agent for LiveAgent {
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
    fn when_idle(&self) -> futures_util::future::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(AbortController::new().signal()))
    }
    fn send(&self, _: hoh_llm::UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn steer(&self, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, message: hoh_llm::UserMessage) -> Result<(), BoxError> {
        lock(&self.injected).push(message);
        Ok(())
    }
}

/// Register the agent behind `agent` in the `agents` registry (once per Session), as the real
/// composition does before any tool call runs. With the approval service mounted, the Session also
/// opens a turn (an approval may only be asked inside one).
pub fn ensure_live(ctx: &Context, agent: &Arc<dyn Agent>) -> Arc<dyn Agent> {
    let session = agent.session();
    let registry = ctx.get::<Agents>().expect("agents registry mounted");
    if let Some(live) = registry.get(session.id()) {
        return live;
    }
    let route = lock(&ROUTES)
        .get(&session.id().to_string())
        .cloned()
        .map(|(provider, model)| AgentOptions {
            provider: Some(Live::Value(provider)),
            model: Some(Live::Value(model)),
            ..AgentOptions::default()
        })
        .unwrap_or_default();
    let live: Arc<dyn hoh_agent::Agent> = Arc::new(LiveAgent {
        id: session.id().clone(),
        session: session.clone(),
        ctx: ctx.clone(),
        scope: agent.scope_key(),
        options: route,
        model: UnusedModel,
        inbox: UnsupportedInbox,
        injected: Mutex::default(),
    });
    // Registered for the kernel's lifetime.
    std::mem::forget(registry.enter(ctx, live.clone()).expect("agent registered"));
    if ctx.get::<hoh_user_approval::Approval>().is_some() {
        session
            .append("turn/start", json!({ "turn": 1 }), None)
            .expect("turn opened");
    }
    live
}

/// The identity stand-in of a test agent: its Session and scope key. A tool call runs with the
/// registered [`LiveAgent`] that [`ensure_live`] returns, which carries the rest of the agent surface.
impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
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
        &UnsupportedInbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        panic!("the tool seam has no context; the registered live agent does")
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> futures_util::future::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(AbortController::new().signal()))
    }
    fn send(&self, _: hoh_llm::UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn steer(&self, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, _: hoh_llm::UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

/// Mounts the real observation policy (`hoh-fs-observation-policy`): it records authoritative
/// observations per owner and target and derives the write, edit and delete guards from them.
pub async fn mount_policy(ctx: &Context) {
    ctx.plugin::<hoh_fs_observation_policy::FsObservationPolicy>(json!({}))
        .expect("observation policy")
        .settled()
        .await
        .expect("observation policy settles");
}

/// A running stack with the fake provider.
pub struct Setup {
    pub kernel: Kernel,
    pub ctx: Context,
    pub fs: Arc<FakeFs>,
    pub tools: hoh_kernel::Service<Tools>,
    pub tool_fs: Fiber,
}

impl Setup {
    /// A standing session override of the sandbox mode (the `sandbox/mode` record of the source).
    pub fn set_standing_mode(&self, agent: &Arc<dyn Agent>, mode: SandboxMode) {
        hoh_sandbox_policy::set_sandbox_mode(agent.session(), mode).expect("sandbox/mode record");
    }
}

/// `setup(config)` of tools.spec.ts: the tool runtime over the fake provider, the policy and the
/// plugin under test with `config`.
pub async fn setup(config: Value) -> Setup {
    try_setup(config).await.expect("setup")
}

pub async fn try_setup(config: Value) -> Result<Setup, hoh_kernel::KernelError> {
    try_setup_with(Options::default(), config).await
}

/// How a test composes the stack around the plugin under test.
#[derive(Default)]
pub struct Options {
    /// The provider confines under this default mode.
    pub confining: Option<SandboxMode>,
    /// Mount the real shared sandbox-policy resolver (needed by a confining provider): the default
    /// mode is `confining` or workspace-write, the fallback project root `/session-project`.
    pub sandbox_policy: bool,
    /// Mount the real approval service with a user answering with this.
    pub approval: Option<Arc<Answerer>>,
}

/// The user behind the real approval service: answers every request with a fixed outcome and counts
/// the requests (a listener on the real `approval/request` event).
pub struct Answerer {
    pub outcome: Mutex<ApprovalOutcome>,
    pub asked: AtomicUsize,
}

impl Answerer {
    pub fn new(outcome: ApprovalOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(outcome),
            asked: AtomicUsize::new(0),
        })
    }
}

pub async fn try_setup_with(
    options: Options,
    config: Value,
) -> Result<Setup, hoh_kernel::KernelError> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fs = Arc::new(FakeFs::default());
    lock(&fs.state)
        .directories
        .insert(format!("key:{DEFAULT_DIRECTORY}"));
    lock(&fs.state).confining = options.confining;
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
    ctx.plugin::<ToolRuntime>(json!({}))?.settled().await?;
    ctx.plugin::<NativeToolExecutorPlugin>(json!({}))?
        .settled()
        .await?;
    ctx.plugin::<WorkingDirectory>(json!({ "defaultDirectory": DEFAULT_DIRECTORY }))?
        .settled()
        .await?;
    mount_policy(&ctx).await;
    if options.sandbox_policy {
        ctx.plugin::<hoh_sandbox_policy::SandboxPolicyService>(json!({
            "mode": options.confining.unwrap_or(SandboxMode::WorkspaceWrite).as_str(),
            "workspaceRoot": "/session-project",
        }))?
        .settled()
        .await?;
    }
    if let Some(answerer) = options.approval.clone() {
        ctx.plugin::<ApprovalService>(json!({}))?.settled().await?;
        ctx.on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            answerer.asked.fetch_add(1, Ordering::SeqCst);
            let outcome = *lock(&answerer.outcome);
            async move { Ok(outcome.into()) }
        })?;
    }
    let tool_fs = ctx.plugin::<ToolFs>(config)?;
    tool_fs.settled().await?;
    let tools = ctx.get::<Tools>().expect("tools service");
    Ok(Setup {
        kernel,
        ctx,
        fs,
        tools,
        tool_fs,
    })
}

/// A signal that never aborts.
pub fn live_signal() -> AbortSignal {
    AbortController::new().signal()
}

static CALLS: AtomicUsize = AtomicUsize::new(1);

/// `executeToolFixture`: drive the invocation stages directly.
pub async fn call(
    ctx: &Context,
    name: &str,
    arguments: Value,
    agent: Option<Arc<dyn Agent>>,
) -> ToolExecutionResult {
    call_with_signal(ctx, name, arguments, agent, live_signal()).await
}

pub async fn call_with_signal(
    ctx: &Context,
    name: &str,
    arguments: Value,
    agent: Option<Arc<dyn Agent>>,
    signal: AbortSignal,
) -> ToolExecutionResult {
    let id: ToolCallId = format!("call-{}", CALLS.fetch_add(1, Ordering::SeqCst))
        .as_str()
        .into();
    let mut input = ToolExecutionInput::new(id, name, arguments, signal);
    input.agent = agent.map(|agent| ensure_live(ctx, &agent));
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

/// The error's `(name, code)` pair, when the result is an error carrying one.
pub fn error_info(result: &ToolExecutionResult) -> Option<(String, String)> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| (info.name.clone(), info.code.clone()))
}

pub fn named(name: &str, code: &str) -> Option<(String, String)> {
    Some((name.to_owned(), code.to_owned()))
}

/// The one text block of a result.
pub fn text_of(result: &ToolExecutionResult) -> String {
    result.text()
}

pub fn harness_error(error: &BoxError) -> Option<&HarnessError> {
    HarnessError::of(error.as_ref())
}

/// The persisted shape of a settled call, the input of a tool's `present_result`.
pub fn to_result(result: &ToolExecutionResult) -> hoh_tools::ToolResult {
    hoh_tools::ToolResult {
        content: result.content().to_vec(),
        is_error: result.is_error(),
        meta: result.meta().cloned(),
    }
}

/// Names of the registered tools, sorted.
pub fn tool_names(setup: &Setup) -> Vec<String> {
    let mut names: Vec<String> = setup
        .tools
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .map(|schema| schema.name)
        .collect();
    names.sort();
    names
}

/// Names of the assembled `tool:` prompt sections, sorted.
pub async fn tool_sections(setup: &Setup) -> Vec<String> {
    let prompt = setup
        .ctx
        .get::<hoh_system_prompt::SystemPromptKey>()
        .expect("system prompt");
    let assembly = prompt
        .assemble(hoh_system_prompt::AssembleContext::default())
        .await
        .unwrap();
    let mut names: Vec<String> = assembly
        .sections
        .iter()
        .map(|section| section.name.clone())
        .filter(|name| name.starts_with("tool:"))
        .collect();
    names.sort();
    names
}
