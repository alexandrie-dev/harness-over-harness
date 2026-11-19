//! Port of @deepseek-ai/dsh-bash-sandbox (packages/shell/bash-sandbox).
//!
//! Sandbox-consuming bash executor. It wraps the exact local bash argv through the `sandbox`
//! service, inherits local process mechanics, and reports the selected mode, enforcement, and
//! denial facts. Positive runner-executable evidence identifies a broken confinement runner:
//! foreground calls fail with `SANDBOX_UNAVAILABLE`, while background processes carry
//! `runnerFailed`; other provider rejections retain stage-neutral local-executor semantics. The
//! tool owns approval and passes a complete per-call policy.

pub mod helpers;

use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use hoh_bash_local::{
    ArgvSource, BashExecution, Config as LocalConfig, ExecuteHooks, LocalBashExecutor,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_sandbox::{
    ConfinedArgv, ConfinedSandboxMode, RunnerFailureRule, Sandbox, SandboxEnforcement,
    SandboxExecutionPolicy, SandboxMode, SandboxPolicy, SandboxUnavailableError,
};
use hoh_sandbox_policy::{SandboxPolicy as PolicyService, SandboxPolicyRequest};
use hoh_shell::{
    ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor, ShellProcess, ShellProcessRead,
    ShellProcessStatus, ShellRunResult, ShellSandboxInfo, provide_shell,
    types::{ShellDone, ShellFuture, ShellObservedStreams},
};
use hoh_subprocess::{Subprocess, SubprocessForeignError, SubprocessSpawnRejection};
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};

use crate::helpers::{
    classify_denial, classify_runner_failure, is_runner_spawn_failure, matches_signature,
};

/// Plugin config: the local executor's knobs, verbatim. The sandbox policy — the default mode and
/// fallback `workspace-write` root — is NOT here: it lives on the `sandboxPolicy` service
/// (`hoh-sandbox-policy`), which resolves each calling session's mode and cwd for every enforcing
/// capability. The runner choice is likewise the `sandbox` provider's config, not this executor's.
pub type Config = LocalConfig;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Mount dependencies of the plugin (`inject = ['subprocess', 'sandbox', 'sandboxPolicy']`).
#[derive(Inject)]
pub struct Deps {
    subprocess: Service<Subprocess>,
    sandbox: Service<Sandbox>,
    policy: Service<PolicyService>,
}

/// Registers as `shell` in place of the local executor and requires a `sandbox` provider plus
/// `sandboxPolicy`; the tool layer is unchanged. Tool calls pass the calling session's resolved
/// policy; direct calls fall back to deployment policy. `result.sandbox` reports the mode and
/// enforcement actually used.
pub struct SandboxBashExecutor {
    local: LocalBashExecutor,
    sandbox: Service<Sandbox>,
    policy: Service<PolicyService>,
    mode: SandboxMode,
}

impl Plugin for SandboxBashExecutor {
    const NAME: &'static str = "SandboxBashExecutor";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let executor = Self::new(config, deps);
        provide_shell(&ctx, Arc::new(executor)).await?;
        Ok(())
    }
}

/// Per-process confinement facts retained until settlement. Providers may vary enforcement and
/// diagnostic dialect between overlapping calls, so a shared latest-wrap value would classify a
/// process against the wrong facts; each call owns its own. Unconfined processes have none.
struct ProcessFacts {
    mode: ConfinedSandboxMode,
    enforcement: SandboxEnforcement,
    denial_signatures: Vec<String>,
    runner_failure_rules: Vec<RunnerFailureRule>,
    runner_program: Option<String>,
    workdir: String,
}

impl SandboxBashExecutor {
    fn new(config: Cfg<Config>, deps: Deps) -> Self {
        // The default mode is the capability fact used for schema advertisement; actual tool
        // executions carry their resolved per-call policy.
        let mode = deps.policy.default_mode;
        Self {
            local: LocalBashExecutor::new(config, deps.subprocess),
            sandbox: deps.sandbox,
            policy: deps.policy,
            mode,
        }
    }
}

/// The mode a confined run uses; `danger-full-access` has no confined form.
fn confined_mode(mode: SandboxMode) -> Option<ConfinedSandboxMode> {
    match mode {
        SandboxMode::ReadOnly => Some(ConfinedSandboxMode::ReadOnly),
        SandboxMode::WorkspaceWrite => Some(ConfinedSandboxMode::WorkspaceWrite),
        SandboxMode::DangerFullAccess => None,
    }
}

fn exit_code_of(code: Option<i64>) -> Option<i32> {
    code.and_then(|code| i32::try_from(code).ok())
}

/// `String(error)` of a thrown value.
fn js_error_string(error: &(dyn std::error::Error + 'static)) -> String {
    let name = HarnessError::of(error).map_or("Error", |harness| harness.name());
    let message = error.to_string();
    if message.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {message}")
    }
}

/// The `code`/`syscall`/`path` a Node spawn error carries, from a `spawn <file> <CODE>` message.
/// Providers name the spawned file in that message; a message of any other shape (for instance an
/// `execve` failure) yields the code alone, which never attributes the failure to a runner.
fn node_facts(code: &Value, message: &str) -> Value {
    let file = code
        .as_str()
        .and_then(|code| {
            message
                .strip_prefix("spawn ")?
                .strip_suffix(code)
                .map(str::trim_end)
        })
        .filter(|file| !file.is_empty());
    match file {
        Some(file) => json!({"code": code, "syscall": format!("spawn {file}"), "path": file}),
        None if message.starts_with("spawn ") => json!({"code": code, "syscall": "spawn"}),
        None => json!({"code": code}),
    }
}

/// The spawn-error facts of the provider's rejection: a direct `SubprocessSpawnRejection` or the
/// `SubprocessForeignError::Coded` a subprocess helper process reports (it keeps the code and the
/// Node-shaped message but not the separate `syscall`/`path`).
fn spawn_error_facts(error: Option<&SharedError>) -> Value {
    let Some(error) = error else {
        return Value::Null;
    };
    let inner: &(dyn std::error::Error + 'static) = error.get_ref();
    let mut current = Some(inner);
    while let Some(candidate) = current {
        if let Some(rejection) = candidate.downcast_ref::<SubprocessSpawnRejection>() {
            return match rejection.code() {
                Some(code) => node_facts(&json!(code), rejection.message()),
                None => Value::Null,
            };
        }
        if let Some(SubprocessForeignError::Coded { message, code }) =
            candidate.downcast_ref::<SubprocessForeignError>()
        {
            return node_facts(code, message);
        }
        if let Some(nested) = candidate.downcast_ref::<SharedError>() {
            current = Some(nested.get_ref());
            continue;
        }
        current = candidate.source();
    }
    Value::Null
}

impl ShellExecutor for SandboxBashExecutor {
    /// The configured default mode — the capability fact the tool layer reads.
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(self.mode)
    }

    /// Stamp a complete per-call policy onto the spec. Tool calls supply the calling session's
    /// resolved mode and root; lower-level callers fall back to the deployment policy.
    // SHIM(hoh-shell): the source `resolve` throws; `ShellExecutor::resolve` returns no `Result`.
    // A deployment policy that cannot be resolved leaves the field empty and `execute` reports it.
    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec {
        let mut spec = self.local.resolve(request);
        spec.sandbox_policy = request
            .sandbox_policy
            .clone()
            .or_else(|| self.policy.resolve(SandboxPolicyRequest::default()).ok());
        spec
    }

    fn execute(&self, spec: ShellExecSpec) -> ShellFuture<'_, Arc<dyn ShellExecution>> {
        Box::pin(async move {
            let policy: SandboxExecutionPolicy = match spec.sandbox_policy.clone() {
                Some(policy) => policy,
                None => self.policy.resolve(SandboxPolicyRequest::default())?,
            };
            let mode = policy.mode;
            let Some(confined_mode) = confined_mode(mode) else {
                let process = self.local.execute_spec(spec).await?;
                return Ok(Arc::new(SandboxExecution::new(
                    process,
                    Decorate::Unconfined { mode },
                )) as Arc<dyn ShellExecution>);
            };
            let confined: Arc<Mutex<Option<ConfinedArgv>>> = Arc::default();
            let facts: Arc<Mutex<Option<ProcessFacts>>> = Arc::default();
            let decorate = Decorate::Confined {
                mode,
                confined_mode,
                confined: confined.clone(),
                signal: spec.signal.clone(),
                workdir: spec.workdir.clone(),
            };
            let sandbox = self.sandbox.clone();
            let command = spec.command.clone();
            let sandbox_policy = SandboxPolicy {
                mode: confined_mode,
                workspace_root: policy.workspace_root.clone(),
                additional_writable_roots: policy.additional_writable_roots.clone(),
                session_id: policy.session_id.clone(),
            };
            let prepared_into = confined.clone();
            let prepare: hoh_bash_local::PrepareArgv = Box::new(move |signal| {
                Box::pin(async move {
                    let argv = vec!["bash".to_owned(), "-c".to_owned(), command];
                    let prepared = sandbox
                        .confine(&argv, sandbox_policy, Some(signal.clone()))
                        .await?;
                    signal.throw_if_aborted()?;
                    let out = prepared.argv.clone();
                    *lock(&prepared_into) = Some(prepared);
                    Ok(out)
                })
            });
            let started_workdir = spec.workdir.clone();
            let started_confined = confined.clone();
            let started_facts = facts.clone();
            let done_facts = facts.clone();
            let hooks = ExecuteHooks {
                on_started: Some(Box::new(move |_process: &BashExecution| {
                    let prepared = lock(&started_confined)
                        .clone()
                        .expect("on_started runs only after confinement completed");
                    *lock(&started_facts) = Some(ProcessFacts {
                        mode: confined_mode,
                        enforcement: prepared.enforcement,
                        denial_signatures: prepared.denial_signatures.clone(),
                        runner_failure_rules: prepared.runner_failure_rules.clone(),
                        runner_program: prepared.argv.first().cloned(),
                        workdir: started_workdir,
                    });
                })),
                on_process_done: Some(Arc::new(
                    move |process, stderr, provider_rejected, error| {
                        stamp_sandbox(&done_facts, process, stderr, provider_rejected, error);
                    },
                )),
            };
            let process = self
                .local
                .execute_argv(spec, ArgvSource::Prepare(prepare), hooks)
                .await?;
            Ok(Arc::new(SandboxExecution::new(process, decorate)) as Arc<dyn ShellExecution>)
        })
    }
}

/// Stamp per-process sandbox facts before `done` settles. Full-access processes have no facts;
/// signal deaths are not denials.
fn stamp_sandbox(
    facts: &Mutex<Option<ProcessFacts>>,
    process: &BashExecution,
    stderr: &str,
    provider_rejected: bool,
    provider_error: Option<&SharedError>,
) {
    let Some(facts) = lock(facts).take() else {
        return;
    };
    let exit_code = exit_code_of(process.exit_code());
    // A provider rejection exposes no public failure stage. Attribute it to the confinement
    // runner only when the error independently names argv[0]. Otherwise settled runner failure
    // outranks denial-like diagnostics.
    let runner_failed = if provider_rejected {
        is_runner_spawn_failure(
            &spawn_error_facts(provider_error),
            facts.runner_program.as_deref(),
            &facts.workdir,
        )
    } else {
        classify_runner_failure(exit_code, stderr, &facts.runner_failure_rules).is_some()
    };
    process.set_sandbox(Some(ShellSandboxInfo {
        mode: facts.mode.into(),
        denied: !runner_failed && matches_signature(exit_code, stderr, &facts.denial_signatures),
        enforcement: Some(facts.enforcement),
        runner_failed: runner_failed.then_some(true),
    }));
}

/// How the foreground projection of one execution is completed.
enum Decorate {
    /// `danger-full-access`: nothing confined the process.
    Unconfined { mode: SandboxMode },
    Confined {
        mode: SandboxMode,
        confined_mode: ConfinedSandboxMode,
        /// The wrap of this call; empty when preparation never completed.
        confined: Arc<Mutex<Option<ConfinedArgv>>>,
        signal: Option<AbortSignal>,
        workdir: String,
    },
}

type Projection = Shared<BoxFuture<'static, Result<ShellRunResult, SharedError>>>;

/// The handle `tool-bash` sees: the local execution plus the sandbox verdict of its foreground
/// projection, computed once.
struct SandboxExecution {
    process: BashExecution,
    decorate: Arc<Decorate>,
    projection: Mutex<Option<Projection>>,
}

impl SandboxExecution {
    fn new(process: BashExecution, decorate: Decorate) -> Self {
        Self {
            process,
            decorate: Arc::new(decorate),
            projection: Mutex::new(None),
        }
    }
}

impl ShellProcess for SandboxExecution {
    fn status(&self) -> ShellProcessStatus {
        self.process.status()
    }
    fn exit_code(&self) -> Option<i64> {
        self.process.exit_code()
    }
    fn signal(&self) -> Option<String> {
        self.process.signal()
    }
    fn done(&self) -> ShellDone {
        self.process.done()
    }
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        self.process.sandbox()
    }
    fn read_output(&self) -> ShellProcessRead {
        self.process.read_output()
    }
    fn observed(&self) -> ShellObservedStreams {
        self.process.observed()
    }
    fn kill(&self) -> bool {
        self.process.kill()
    }
}

impl ShellExecution for SandboxExecution {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        let shared = lock(&self.projection)
            .get_or_insert_with(|| {
                let process = self.process.clone();
                let decorate = self.decorate.clone();
                // `decorated ??= base().then(map, mapError)`: one projection, shared by every call.
                async move {
                    match process.result().await {
                        Ok(result) => decorate.map(result),
                        Err(error) => Err(decorate.map_error(error).await),
                    }
                }
                .boxed()
                .shared()
            })
            .clone();
        Box::pin(async move { shared.await.map_err(|error| Box::new(error) as BoxError) })
    }
}

fn shared(error: BoxError) -> SharedError {
    match error.downcast::<SharedError>() {
        Ok(shared) => *shared,
        Err(error) => SharedError::new(error),
    }
}

impl Decorate {
    fn map(&self, result: ShellRunResult) -> Result<ShellRunResult, SharedError> {
        match self {
            Self::Unconfined { mode } => Ok(ShellRunResult {
                sandbox: Some(ShellSandboxInfo {
                    mode: *mode,
                    denied: false,
                    enforcement: None,
                    runner_failed: None,
                }),
                ..result
            }),
            Self::Confined {
                mode,
                confined_mode,
                confined,
                ..
            } => {
                let Some(wrap) = lock(confined).clone() else {
                    return Ok(ShellRunResult {
                        sandbox: Some(ShellSandboxInfo {
                            mode: *mode,
                            denied: false,
                            enforcement: None,
                            runner_failed: None,
                        }),
                        ..result
                    });
                };
                // Runner failure outranks denial because the command did not run. Carry the
                // matched fatal line, not an informational line that preceded it.
                if let Some(failure) = classify_runner_failure(
                    exit_code_of(result.exit_code),
                    &result.stderr.text,
                    &wrap.runner_failure_rules,
                ) {
                    return Err(SharedError::new(SandboxUnavailableError::new(
                        *confined_mode,
                        Some(&failure.detail),
                    )));
                }
                let denied = classify_denial(&result, &wrap.denial_signatures);
                Ok(ShellRunResult {
                    sandbox: Some(ShellSandboxInfo {
                        mode: *mode,
                        denied,
                        enforcement: Some(wrap.enforcement),
                        runner_failed: None,
                    }),
                    ..result
                })
            }
        }
    }

    async fn map_error(&self, error: BoxError) -> SharedError {
        let error = shared(error);
        let Self::Confined {
            confined_mode,
            confined,
            signal,
            workdir,
            ..
        } = self
        else {
            return error;
        };
        // An upstream abort remains cancellation even when it prevents spawn.
        if let Some(signal) = signal
            && let Err(reason) = signal.throw_if_aborted()
        {
            return reason;
        }
        let runner = lock(confined)
            .as_ref()
            .map(|wrap| wrap.argv.first().cloned());
        if let Some(runner) = runner
            && is_runner_spawn_failure(&spawn_error_facts(Some(&error)), runner.as_deref(), workdir)
        {
            return SharedError::new(SandboxUnavailableError::new(
                *confined_mode,
                Some(&js_error_string(error.get_ref())),
            ));
        }
        error
    }
}
