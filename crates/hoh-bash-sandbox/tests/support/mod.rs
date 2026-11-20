//! Shared fixtures: a recording fake `sandbox` provider, a `subprocess` runtime that delegates to
//! the real local one unless a test scripts `spawn`, and the composition around the executor.
#![allow(dead_code)]

use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use futures_util::future::BoxFuture;
use hoh_bash_sandbox::SandboxBashExecutor;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Fiber, Kernel, Service};
use hoh_sandbox::{
    ConfinedArgv, RunnerFailureRule, Sandbox, SandboxEnforcement, SandboxFuture, SandboxPolicy,
    SandboxProvider,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_shell::{
    Shell, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExpiryPolicy, ShellRunResult,
};
use hoh_subprocess::{
    Subprocess, SubprocessForeignError, SubprocessFuture, SubprocessHandle, SubprocessRuntime,
    SubprocessSpawnRejection, SubprocessSpawnSpec, SubprocessTerminalEnvironment,
    SubprocessTerminalHandle, SubprocessTerminalSpawnSpec, provide_subprocess,
};
use hoh_subprocess_local::LocalSubprocessRuntime;
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The Linux file-denial dialects the fake wraps carry — matches the unix-permission denials the
/// tests produce.
pub fn unix_signatures() -> Vec<String> {
    vec!["read-only file system".into(), "permission denied".into()]
}

/// The runner-failure rule the fake wraps carry (a `fake-runner:` error line marks the sandbox
/// itself failing).
pub fn runner_failure() -> Vec<RunnerFailureRule> {
    vec![RunnerFailureRule {
        allowed_exit_codes: None,
        fatal_signatures: vec!["fake-runner: ".into()],
        informational_lines: None,
    }]
}

/// A wrap with the fake's usual facts around `argv`.
pub fn wrap(argv: Vec<String>) -> ConfinedArgv {
    ConfinedArgv {
        argv,
        enforcement: SandboxEnforcement::Full,
        denial_signatures: unix_signatures(),
        runner_failure_rules: runner_failure(),
    }
}

/// A passthrough wrap: the caller's argv unchanged, asserted full — commands run unconfined,
/// deterministically.
pub fn passthrough(argv: &[String]) -> ConfinedArgv {
    wrap(argv.to_vec())
}

pub fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// One recorded provider call: the argv handed over and the policy it rode with.
#[derive(Clone, Debug)]
pub struct ConfineCall {
    pub argv: Vec<String>,
    pub policy: SandboxPolicy,
}

pub type Behavior = Arc<
    dyn Fn(
            &[String],
            &SandboxPolicy,
            Option<AbortSignal>,
        ) -> BoxFuture<'static, Result<ConfinedArgv, BoxError>>
        + Send
        + Sync,
>;

/// A behavior that answers at once.
pub fn immediate(
    answer: impl Fn(&[String], &SandboxPolicy) -> Result<ConfinedArgv, BoxError> + Send + Sync + 'static,
) -> Behavior {
    Arc::new(move |argv, policy, _| {
        let answer = answer(argv, policy);
        Box::pin(async move { answer })
    })
}

pub fn passthrough_behavior() -> Behavior {
    immediate(|argv, _| Ok(passthrough(argv)))
}

/// The recording fake `sandbox` service.
pub struct FakeSandbox {
    pub calls: Mutex<Vec<ConfineCall>>,
    behavior: Behavior,
}

impl SandboxProvider for FakeSandbox {
    fn confine<'a>(
        &'a self,
        argv: &'a [String],
        policy: SandboxPolicy,
        signal: Option<AbortSignal>,
    ) -> SandboxFuture<'a, ConfinedArgv> {
        lock(&self.calls).push(ConfineCall {
            argv: argv.to_vec(),
            policy: policy.clone(),
        });
        (self.behavior)(argv, &policy, signal)
    }
}

impl FakeSandbox {
    pub fn calls(&self) -> Vec<ConfineCall> {
        lock(&self.calls).clone()
    }
}

/// What `spawn` does: delegate to the real local runtime, throw, or hand out a scripted handle.
#[derive(Clone)]
pub enum SpawnMode {
    Real,
    Throw(Arc<dyn Fn() -> BoxError + Send + Sync>),
    Handle(Arc<dyn Fn(&SubprocessSpawnSpec) -> SubprocessHandle + Send + Sync>),
}

#[derive(Clone, Debug)]
pub struct SpawnRecord {
    pub argv: Vec<String>,
    pub signal: Option<AbortSignal>,
    pub cwd: String,
}

/// `vi.spyOn(ctx.subprocess, 'spawn')`: the real local runtime with a scriptable `spawn`.
pub struct ScriptedSubprocess {
    inner: LocalSubprocessRuntime,
    mode: Mutex<SpawnMode>,
    pub spawns: Mutex<Vec<SpawnRecord>>,
}

impl ScriptedSubprocess {
    pub fn set(&self, mode: SpawnMode) {
        *lock(&self.mode) = mode;
    }

    pub fn spawn_count(&self) -> usize {
        lock(&self.spawns).len()
    }

    pub fn spawns(&self) -> Vec<SpawnRecord> {
        lock(&self.spawns).clone()
    }
}

impl SubprocessRuntime for ScriptedSubprocess {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        env: Option<&'a indexmap::IndexMap<String, String>>,
        signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        self.inner.resolve_executable(command, env, signal)
    }

    fn terminal_environment(
        &self,
        signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        self.inner.terminal_environment(signal)
    }

    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        lock(&self.spawns).push(SpawnRecord {
            argv: spec.argv.clone(),
            signal: spec.signal.clone(),
            cwd: spec.cwd.clone(),
        });
        let mode = lock(&self.mode).clone();
        match mode {
            SpawnMode::Real => self.inner.spawn(spec),
            SpawnMode::Throw(error) => Err(error()),
            SpawnMode::Handle(handle) => Ok(handle(&spec)),
        }
    }

    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        self.inner.spawn_terminal(spec)
    }
}

/// A Node-shaped spawn failure of `file`: `spawn <file> <CODE>`.
pub fn spawn_failure(errno: i32, file: &str) -> BoxError {
    Box::new(SubprocessSpawnRejection::new(
        io::Error::from_raw_os_error(errno),
        file,
    ))
}

/// The errno code of a (possibly shared) provider failure.
pub fn error_code(error: &BoxError) -> Option<String> {
    fn find(error: &(dyn std::error::Error + 'static), depth: usize) -> Option<String> {
        if depth == 16 {
            return None;
        }
        if let Some(rejection) = error.downcast_ref::<SubprocessSpawnRejection>() {
            return rejection.code().map(str::to_owned);
        }
        if let Some(SubprocessForeignError::Coded { code, .. }) = error.downcast_ref() {
            return code.as_str().map(str::to_owned);
        }
        if let Some(shared) = error.downcast_ref::<hoh_error::SharedError>() {
            return find(shared.get_ref(), depth + 1);
        }
        error.source().and_then(|source| find(source, depth + 1))
    }

    find(error.as_ref(), 0)
}

/// The `name` and `code` of a harness error, however the error is shared.
pub fn harness_name_code(error: &BoxError) -> Option<(String, String)> {
    let inner: &(dyn std::error::Error + 'static) =
        match error.downcast_ref::<hoh_error::SharedError>() {
            Some(shared) => shared.get_ref(),
            None => error.as_ref(),
        };
    HarnessError::of(inner).map(|harness| (harness.name().to_owned(), harness.code().to_owned()))
}

pub fn is_sandbox_unavailable(error: &BoxError) -> bool {
    harness_name_code(error).is_some_and(|(name, code)| {
        name == "SandboxUnavailableError" && code == "SANDBOX_UNAVAILABLE"
    })
}

/// The composition under test.
pub struct Setup {
    pub kernel: Kernel,
    pub ctx: Context,
    pub shell: Service<Shell>,
    pub sandbox: Arc<FakeSandbox>,
    pub subprocess: Arc<ScriptedSubprocess>,
    pub executor: Fiber,
    pub spill: tempfile::TempDir,
}

impl Setup {
    pub fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        self.shell.resolve(&request)
    }

    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}

pub fn request(command: &str) -> ShellExecRequest {
    ShellExecRequest {
        command: command.to_owned(),
        workdir: None,
        timeout_ms: None,
        on_expiry: None,
        stdout_max_bytes: None,
        signal: None,
        stdin: None,
        env: None,
        dsh_env: None,
        sandbox_policy: None,
    }
}

/// Historical foreground shorthand over the unified `execute()` seam.
pub async fn run(setup: &Setup, spec: ShellExecSpec) -> Result<ShellRunResult, BoxError> {
    setup.shell.execute(spec).await?.result().await
}

/// Historical background shorthand: execute with no deadline armed.
pub async fn start(
    setup: &Setup,
    spec: ShellExecSpec,
) -> Result<Arc<dyn ShellExecution>, BoxError> {
    setup
        .shell
        .execute(ShellExecSpec {
            on_expiry: ShellExpiryPolicy::None,
            ..spec
        })
        .await
}

/// Which services the composition mounts around the executor.
pub struct Options {
    pub mode: Option<&'static str>,
    pub workspace_root: Option<String>,
    pub executor: Value,
    pub behavior: Behavior,
    pub protected_roots: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: None,
            workspace_root: None,
            executor: json!({}),
            behavior: passthrough_behavior(),
            protected_roots: Vec::new(),
        }
    }
}

/// Boot a kernel with a recording fake `sandbox` (behavior injectable per test), the real policy
/// service, a scriptable subprocess runtime and the executor under test on top of them.
pub async fn setup(options: Options) -> Setup {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sandbox = Arc::new(FakeSandbox {
        calls: Mutex::default(),
        behavior: options.behavior,
    });
    ctx.provide::<Sandbox>(sandbox.clone()).await.unwrap();
    let mut policy = json!({});
    if let Some(mode) = options.mode {
        policy["mode"] = json!(mode);
    }
    if let Some(root) = &options.workspace_root {
        policy["workspaceRoot"] = json!(root);
    }
    policy["protectedRoots"] = json!(options.protected_roots);
    ctx.plugin::<hoh_sandbox_policy::SandboxPolicyService>(policy)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let spill = tempfile::tempdir().unwrap();
    let local = LocalSubprocessRuntime::new(ctx.clone()).unwrap();
    local.internals.lock().unwrap().spill_dir = Some(PathBuf::from(spill.path()));
    let subprocess = Arc::new(ScriptedSubprocess {
        inner: local,
        mode: Mutex::new(SpawnMode::Real),
        spawns: Mutex::default(),
    });
    provide_subprocess(&ctx, subprocess.clone()).await.unwrap();
    let mut config = json!({"graceMs": 200});
    if let (Some(extra), Some(target)) = (options.executor.as_object(), config.as_object_mut()) {
        target.extend(extra.clone());
    }
    let executor = ctx.plugin::<SandboxBashExecutor>(config).unwrap();
    executor.settled().await.unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    Setup {
        kernel,
        ctx,
        shell,
        sandbox,
        subprocess,
        executor,
        spill,
    }
}

pub fn handle_unused(_: &Subprocess) {}

/// A composition over a REAL `sandbox` provider and the real local subprocess runtime — nothing
/// scripted or recorded.
pub struct Real {
    pub kernel: Kernel,
    pub shell: Service<Shell>,
    pub spill: tempfile::TempDir,
}

impl Real {
    pub fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        self.shell.resolve(&request)
    }

    pub async fn close(self) {
        self.kernel.shutdown().await;
    }

    pub async fn run(&self, spec: ShellExecSpec) -> Result<ShellRunResult, BoxError> {
        self.shell.execute(spec).await?.result().await
    }

    pub async fn start(&self, spec: ShellExecSpec) -> Result<Arc<dyn ShellExecution>, BoxError> {
        self.shell
            .execute(ShellExecSpec {
                on_expiry: ShellExpiryPolicy::None,
                ..spec
            })
            .await
    }
}

/// Boot the real policy service, `provider` as the `sandbox` service, the local subprocess runtime
/// and the executor with `executor` config.
pub async fn setup_real(
    provider: Arc<dyn SandboxProvider>,
    executor: Value,
    mode: &str,
    workspace_root: &str,
) -> Real {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.provide::<Sandbox>(provider).await.unwrap();
    ctx.plugin::<hoh_sandbox_policy::SandboxPolicyService>(json!({
        "mode": mode,
        "workspaceRoot": workspace_root,
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    let spill = tempfile::tempdir().unwrap();
    let local = LocalSubprocessRuntime::new(ctx.clone()).unwrap();
    local.internals.lock().unwrap().spill_dir = Some(PathBuf::from(spill.path()));
    provide_subprocess(&ctx, Arc::new(local)).await.unwrap();
    ctx.plugin::<SandboxBashExecutor>(executor)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    Real {
        kernel,
        shell,
        spill,
    }
}
