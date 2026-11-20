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
    mode: SandboxMode,
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

/// The process's own signal that its runner finished the path-based setup: the confined command's
/// first act is to create `ready` in a directory the runner binds into the sandbox. Until then the
/// protected-effect lock stays held, so no other tool call can swap a checked path under the
/// setup; after it the lock goes back at once instead of waiting for the process to end, which a
/// long or backgrounded command may never do. A runner that never signals keeps the lock until the
/// process ends (fail closed): no timer ever releases it.
struct ReadyMarker {
    dir: std::path::PathBuf,
}

/// The per-user directory under which every call gets its own private subdirectory: the runtime
/// directory, else the state directory, else (last resort) a per-user directory in the temporary
/// one. Never a shared, predictable place: callers verify owner and mode before using it.
fn ready_base() -> std::path::PathBuf {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if let Some(runtime) = absolute("XDG_RUNTIME_DIR") {
        return runtime.join("hoh-ready");
    }
    if let Some(state) = absolute("XDG_STATE_HOME") {
        return state.join("hoh").join("ready");
    }
    if let Some(home) = absolute("HOME") {
        return home.join(".local").join("state").join("hoh").join("ready");
    }
    std::env::temp_dir().join(format!("hoh-ready-{}", rustix::process::geteuid().as_raw()))
}

/// `path` is a real directory (not a link) owned by this user that nobody else can enter.
fn require_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory of this user", path.display()),
        ));
    }
    Ok(())
}

/// 128 random bits as hex, from the system generator.
fn random_name() -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `text` as one shell word in single quotes.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

impl ReadyMarker {
    fn create() -> std::io::Result<Self> {
        Self::create_in(&ready_base())
    }

    fn create_in(base: &std::path::Path) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(base)?;
        // The directory is bind-mounted by path: a spelling through a symlink (a `HOME` or state
        // directory reached by a link) cannot be created inside the sandbox, so use the real path.
        let base = std::fs::canonicalize(base)?;
        require_private_dir(&base)?;
        let dir = base.join(random_name()?);
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        require_private_dir(&dir)?;
        Ok(Self { dir })
    }

    fn dir_text(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    /// `command` preceded by the signal. Same first line, so `bash: line 1:` reports stay as they were.
    fn wrap(&self, command: &str) -> String {
        let flag = self.dir.join("ready");
        format!(
            ": > {} 2>/dev/null; {command}",
            shell_quote(&flag.to_string_lossy())
        )
    }

    /// The signal is a regular file of this user; anything else in its place is not the command's.
    fn signalled(dir: &std::path::Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(dir.join("ready")).is_ok_and(|metadata| {
            metadata.is_file() && metadata.uid() == rustix::process::geteuid().as_raw()
        })
    }

    /// Give the lock back when the signal appears, or drop the directory when the process ended
    /// first (its own settlement then released the lock).
    fn release_on_signal(self, guard: Arc<Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>>) {
        tokio::spawn(async move {
            loop {
                if lock(&guard).is_none() {
                    break;
                }
                if Self::signalled(&self.dir) {
                    lock(&guard).take();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
    }
}

impl Drop for ReadyMarker {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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
            let mut policy: SandboxExecutionPolicy = match spec.sandbox_policy.clone() {
                Some(policy) => policy,
                None => self.policy.resolve(SandboxPolicyRequest::default())?,
            };
            policy.protected_roots = (!self.policy.protected_roots().is_empty())
                .then(|| self.policy.protected_roots().to_vec());
            // Waiting for another model effect is part of this call: its signal and deadline end
            // the wait, and an already-cancelled call never queues.
            let protected_guard = Arc::new(Mutex::new(
                self.policy
                    .lock_protected_effect_within(spec.signal.as_ref(), Some(spec.timeout_ms))
                    .await?,
            ));
            self.policy.validate_protected_root_identities().await?;
            let mode = policy.mode;
            let protected = policy
                .protected_roots
                .as_ref()
                .is_some_and(|roots| !roots.is_empty());
            let confined_mode = confined_mode(mode).or_else(|| {
                // Deployment-protected roots are non-escalatable. Represent otherwise-unlimited
                // writes as workspace-write rooted at `/`; sandbox-local then subtracts its
                // matching protectedRoots before the command starts.
                protected.then_some(ConfinedSandboxMode::WorkspaceWrite)
            });
            let Some(confined_mode) = confined_mode else {
                let process = self.local.execute_spec(spec).await?;
                return Ok(Arc::new(SandboxExecution::new(
                    process,
                    Decorate::Unconfined { mode },
                    protected_guard,
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
            let ready = if protected {
                Some(ReadyMarker::create()?)
            } else {
                None
            };
            let sandbox = self.sandbox.clone();
            let command = match &ready {
                Some(ready) => ready.wrap(&spec.command),
                None => spec.command.clone(),
            };
            let sandbox_policy = SandboxPolicy {
                mode: confined_mode,
                workspace_root: if mode == SandboxMode::DangerFullAccess && protected {
                    "/".into()
                } else {
                    policy.workspace_root.clone()
                },
                additional_writable_roots: (mode != SandboxMode::DangerFullAccess)
                    .then(|| policy.additional_writable_roots.clone())
                    .flatten(),
                session_id: policy.session_id.clone(),
                protected_roots: policy.protected_roots.clone(),
                unrestricted_writes: mode == SandboxMode::DangerFullAccess,
                ready_dir: ready.as_ref().map(ReadyMarker::dir_text),
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
            let done_guard = protected_guard.clone();
            let hooks = ExecuteHooks {
                on_started: Some(Box::new(move |_process: &BashExecution| {
                    let prepared = lock(&started_confined)
                        .clone()
                        .expect("on_started runs only after confinement completed");
                    *lock(&started_facts) = Some(ProcessFacts {
                        mode,
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
                        lock(&done_guard).take();
                    },
                )),
            };
            let process = self
                .local
                .execute_argv(spec, ArgvSource::Prepare(prepare), hooks)
                .await?;
            if let Some(ready) = ready {
                ready.release_on_signal(protected_guard.clone());
            }
            Ok(
                Arc::new(SandboxExecution::new(process, decorate, protected_guard))
                    as Arc<dyn ShellExecution>,
            )
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
        mode: facts.mode,
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
    _protected_guard: Arc<Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>>,
}

impl SandboxExecution {
    fn new(
        process: BashExecution,
        decorate: Decorate,
        protected_guard: Arc<Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>>,
    ) -> Self {
        Self {
            process,
            decorate: Arc::new(decorate),
            projection: Mutex::new(None),
            _protected_guard: protected_guard,
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

#[cfg(test)]
mod ready_marker_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn base() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn the_directory_name_is_random_and_the_directory_private() {
        let base = base();
        let a = ReadyMarker::create_in(base.path()).unwrap();
        let b = ReadyMarker::create_in(base.path()).unwrap();
        let name = |m: &ReadyMarker| m.dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_ne!(name(&a), name(&b));
        assert_eq!(name(&a).len(), 32);
        assert!(name(&a).bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!name(&a).contains(&std::process::id().to_string()));
        assert_eq!(
            std::fs::metadata(&a.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn the_default_base_is_not_a_shared_temporary_directory() {
        let base = ready_base();
        assert!(base.is_absolute());
        // Only the last-resort branch lives under the temporary directory, and it is per user.
        if base.starts_with(std::env::temp_dir()) {
            assert!(base.to_string_lossy().contains("hoh-ready-"));
        }
    }

    #[test]
    fn a_base_other_users_can_enter_is_refused() {
        let base = base();
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = ReadyMarker::create_in(base.path()).err().expect("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_forged_marker_that_is_not_a_regular_file_does_not_signal() {
        let base = base();
        let marker = ReadyMarker::create_in(base.path()).unwrap();
        assert!(!ReadyMarker::signalled(&marker.dir));
        symlink("/dev/null", marker.dir.join("ready")).unwrap();
        assert!(!ReadyMarker::signalled(&marker.dir));
        std::fs::remove_file(marker.dir.join("ready")).unwrap();
        std::fs::create_dir(marker.dir.join("ready")).unwrap();
        assert!(!ReadyMarker::signalled(&marker.dir));
        std::fs::remove_dir(marker.dir.join("ready")).unwrap();
        std::fs::write(marker.dir.join("ready"), "").unwrap();
        assert!(ReadyMarker::signalled(&marker.dir));
    }

    /// A state directory reached through a symlink gives a marker directory at its real path, the
    /// spelling a sandbox runner can mount.
    #[test]
    fn a_base_reached_through_a_symlink_yields_a_real_path() {
        let outer = base();
        let real = outer.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = outer.path().join("link");
        symlink(&real, &link).unwrap();
        let marker = ReadyMarker::create_in(&link).unwrap();
        assert_eq!(
            marker.dir.parent().unwrap(),
            std::fs::canonicalize(&real).unwrap()
        );
    }

    /// A base directory whose path holds a single quote must not break the wrapped command.
    #[test]
    fn a_quote_in_the_base_path_leaves_the_command_intact() {
        let outer = base();
        let quoted = outer.path().join("it's a 'dir'");
        let marker = ReadyMarker::create_in(&quoted).unwrap();
        let wrapped = marker.wrap("printf 'out:%s' \"$((1+1))\"");
        let output = std::process::Command::new("bash")
            .args(["-c", &wrapped])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out:2");
        assert!(ReadyMarker::signalled(&marker.dir));
    }
}
