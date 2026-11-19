//! Port of @deepseek-ai/dsh-bash-local (packages/shell/bash-local).
//!
//! Local bash executor over the `subprocess` service. Public commands run as `bash -c` through
//! `Subprocess::spawn`; the executor owns command defaulting, deadlines and cause classification, the
//! model-friendly terminal environment and the model-facing stdout/stderr merge for background reads.

mod execution;

use std::sync::Arc;

use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Volatile};
use hoh_shell::{
    ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor, ShellExpiryPolicy,
    provide_shell, types::ShellFuture,
};
use hoh_subprocess::{
    Subprocess, SubprocessCollect, SubprocessOutputMode, SubprocessSpawnSpec, SubprocessSpill,
    SubprocessStdinMode, SubprocessStdio,
};
use hoh_timeout::{AbortSignal, MAX_TIMER_DELAY_MS};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use execution::{
    ArgvFuture, ArgvSource, BashExecution, ExecuteHooks, PrepareArgv, ProcessDoneHook,
};

/// Model-friendly environment overrides: disable colors, pagers and interactive terminal features
/// that would garble tool output. Merged first into the spawn's explicit env, so a trusted caller's
/// own entry still wins.
pub const ENV_OVERRIDES: [(&str, &str); 4] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
];

/// Default SIGTERM→SIGKILL grace period (the `graceMs` config).
const DEFAULT_GRACE_MS: f64 = 3_000.0;

/// Default per-stream spill cap (the `maxSpillBytes` config).
const DEFAULT_MAX_SPILL_BYTES: f64 = 64.0 * 1024.0 * 1024.0;

fn default_timeout_ms() -> Volatile<f64> {
    Volatile(120_000.0)
}
fn default_max_timeout_ms() -> Volatile<f64> {
    Volatile(600_000.0)
}
fn default_max_output_bytes() -> Volatile<f64> {
    Volatile(64_000.0)
}
fn default_max_spill_bytes() -> Volatile<f64> {
    Volatile(DEFAULT_MAX_SPILL_BYTES)
}
fn default_grace_ms() -> Volatile<f64> {
    Volatile(DEFAULT_GRACE_MS)
}

/// Validated plugin configuration with live command budgets.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
#[schemars(extend("default" = serde_json::json!({})))]
pub struct Config {
    /// Default working directory for commands (default: the process working directory).
    pub cwd: Volatile<Option<String>>,
    /// Default foreground timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: Volatile<f64>,
    /// Upper bound for per-call timeout overrides.
    #[serde(default = "default_max_timeout_ms")]
    pub max_timeout_ms: Volatile<f64>,
    /// Per-stream in-memory output cap; overflow spills to a temp file.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: Volatile<f64>,
    /// Per-stream spill-file cap; larger streams retain only their in-memory tail.
    #[serde(default = "default_max_spill_bytes")]
    pub max_spill_bytes: Volatile<f64>,
    /// Grace period for kill escalation and inherited pipes; at most `MAX_TIMER_DELAY_MS`.
    #[serde(default = "default_grace_ms")]
    pub grace_ms: Volatile<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cwd: Volatile(None),
            timeout_ms: default_timeout_ms(),
            max_timeout_ms: default_max_timeout_ms(),
            max_output_bytes: default_max_output_bytes(),
            max_spill_bytes: default_max_spill_bytes(),
            grace_ms: default_grace_ms(),
        }
    }
}

/// The two `Error`s `assertServiceableBashConfig` and `resolve` throw; texts are model/user visible.
#[derive(Debug, thiserror::Error)]
pub enum BashConfigError {
    #[error("bash-local: {0} must be a positive finite number")]
    NotPositiveFinite(&'static str),
    #[error("bash-local: graceMs must be no greater than {MAX_TIMER_DELAY_MS}")]
    GraceTooLarge,
}

/// Bytes the TS output collector retains for a (possibly fractional) cap: its trim removes
/// `trunc(length - cap)` bytes from an over-cap chunk, leaving `ceil(cap)` (`0.5` keeps one byte).
fn output_cap(configured: f64) -> u64 {
    configured.ceil() as u64
}

fn assert_positive_finite(name: &'static str, value: f64) -> Result<(), BashConfigError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(BashConfigError::NotPositiveFinite(name));
    }
    Ok(())
}

/// Reject a resolved section this executor could not run with. The schema expresses neither
/// "positive and finite" nor the timer bound `graceMs` has to fit, so a stored value that cannot be
/// used fails at the next command.
pub fn assert_serviceable_bash_config(config: &Config) -> Result<(), BashConfigError> {
    assert_positive_finite("timeoutMs", config.timeout_ms.0)?;
    assert_positive_finite("maxTimeoutMs", config.max_timeout_ms.0)?;
    assert_positive_finite("maxOutputBytes", config.max_output_bytes.0)?;
    assert_positive_finite("maxSpillBytes", config.max_spill_bytes.0)?;
    assert_positive_finite("graceMs", config.grace_ms.0)?;
    if config.grace_ms.0 > MAX_TIMER_DELAY_MS as f64 {
        return Err(BashConfigError::GraceTooLarge);
    }
    Ok(())
}

#[derive(Inject)]
pub struct Deps {
    subprocess: Service<Subprocess>,
}

/// Local bash executor over the `subprocess` service. Bounded output, spill files, managed-range
/// SIGTERM→SIGKILL escalation and quiescence are the subprocess service's mechanics; this executor
/// supplies their configured budgets per spawn, so a still-running background process stays managed
/// (killed and joined at composition teardown) even across an executor reload.
pub struct LocalBashExecutor {
    config: Cfg<Config>,
    subprocess: Service<Subprocess>,
}

impl Plugin for LocalBashExecutor {
    const NAME: &'static str = "LocalBashExecutor";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let executor = Self {
            config,
            subprocess: deps.subprocess,
        };
        provide_shell(&ctx, Arc::new(executor)).await?;
        Ok(())
    }
}

impl LocalBashExecutor {
    /// Compose an executor over a resolved section and the `subprocess` service, for subclasses
    /// (sandboxing executors) that drive `execute_argv` themselves.
    pub fn new(config: Cfg<Config>, subprocess: Service<Subprocess>) -> Self {
        Self { config, subprocess }
    }

    /// Resolve a request into a fully-specified spec: fill `workdir` from `config.cwd` (else the
    /// process working directory) and `timeoutMs` from `config.timeoutMs`, capped at
    /// `config.maxTimeoutMs`. The source throws on an unusable budget; this method returns the same
    /// error text as `Err`.
    pub fn try_resolve(&self, request: &ShellExecRequest) -> Result<ShellExecSpec, BoxError> {
        let config = self.config.get();
        assert_serviceable_bash_config(&config)?;
        let timeout_ms = hoh_timeout::clamp_timeout(
            request.timeout_ms,
            config.timeout_ms.0,
            config.max_timeout_ms.0,
            Some("bash-local: request.timeoutMs"),
        )?;
        // The seam types bytes as u64: validate the configured number as the source does, then
        // truncate it (a fractional cap is accepted; PORT_NOTES).
        let stdout_max_bytes = match request.stdout_max_bytes {
            Some(bytes) => {
                assert_positive_finite("request.stdoutMaxBytes", bytes as f64)?;
                bytes
            }
            None => {
                assert_positive_finite("request.stdoutMaxBytes", config.max_output_bytes.0)?;
                output_cap(config.max_output_bytes.0)
            }
        };
        let workdir = match request.workdir.clone().or_else(|| config.cwd.0.clone()) {
            Some(workdir) => workdir,
            None => std::env::current_dir()?.to_string_lossy().into_owned(),
        };
        Ok(ShellExecSpec {
            command: request.command.clone(),
            workdir,
            timeout_ms,
            on_expiry: request.on_expiry.unwrap_or(ShellExpiryPolicy::Kill),
            stdout_max_bytes,
            signal: request.signal.clone(),
            // Carry stdin/ordinary env/trusted dshEnv through verbatim — optional, no config default.
            stdin: request.stdin.clone(),
            env: request.env.clone(),
            dsh_env: request.dsh_env.clone(),
            // This executor never confines, so the field is inert here (the seam contract).
            sandbox_policy: request.sandbox_policy.clone(),
        })
    }

    /// Execute `bash -c <command>` with the lifecycle, environment, output, deadline and
    /// cancellation semantics of this executor.
    pub async fn execute_spec(&self, spec: ShellExecSpec) -> Result<BashExecution, BoxError> {
        let argv = vec!["bash".to_owned(), "-c".to_owned(), spec.command.clone()];
        self.execute_argv(spec, ArgvSource::Ready(argv), ExecuteHooks::default())
            .await
    }

    /// Map one resolved bash spec and explicit argv onto a fully-specified subprocess spawn.
    fn spawn_spec(
        &self,
        spec: &ShellExecSpec,
        argv: Vec<String>,
        signal: Option<AbortSignal>,
    ) -> SubprocessSpawnSpec {
        let config = self.config.get();
        let collect = |max_bytes: u64| {
            SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes,
                spill: Some(SubprocessSpill {
                    max_bytes: config.max_spill_bytes.0 as u64,
                }),
            })
        };
        // One explicit env map for the seam, layered so the trusted dshEnv snapshot beats both the
        // caller's env and the terminal overrides.
        let mut env = IndexMap::new();
        for (name, value) in ENV_OVERRIDES {
            env.insert(name.to_owned(), Some(value.to_owned()));
        }
        for (name, value) in spec.env.iter().flatten() {
            env.insert(name.clone(), Some(value.clone()));
        }
        for (name, value) in spec.dsh_env.iter().flatten() {
            env.insert(name.clone(), Some(value.clone()));
        }
        SubprocessSpawnSpec {
            argv,
            cwd: spec.workdir.clone(),
            stdio: SubprocessStdio {
                stdin: match &spec.stdin {
                    Some(data) => SubprocessStdinMode::Data { data: data.clone() },
                    None => SubprocessStdinMode::Ignore,
                },
                stdout: collect(spec.stdout_max_bytes),
                stderr: collect(output_cap(config.max_output_bytes.0)),
                control: None,
            },
            grace_ms: config.grace_ms.0,
            signal,
            env: Some(env),
        }
    }

    /// Execute an explicit argv with the lifecycle, environment, output, deadline and cancellation
    /// semantics of this executor. Subclasses use this after replacing the public command's shell
    /// argv at an execution boundary.
    ///
    /// `argv` is the exact argv or a preparation using the execution cancellation signal;
    /// `hooks.on_started` installs provider facts synchronously before the handle can settle, and
    /// `hooks.on_process_done` is the settlement hook of the source's protected `onProcessDone`.
    /// Spawn rejection settles the handle as killed while `result()` carries the same failure.
    pub async fn execute_argv(
        &self,
        spec: ShellExecSpec,
        argv: ArgvSource,
        hooks: ExecuteHooks,
    ) -> Result<BashExecution, BoxError> {
        execution::launch(self, spec, argv, hooks).await
    }
}

impl ShellExecutor for LocalBashExecutor {
    // SHIM(hoh-shell): the source `resolve` throws on an unusable budget, but
    // `ShellExecutor::resolve` returns no `Result`; the failure surfaces as a panic with the exact
    // source message. Replace by `try_resolve` when hoh-shell makes `resolve` fallible.
    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec {
        match self.try_resolve(request) {
            Ok(spec) => spec,
            Err(error) => panic!("{error}"),
        }
    }

    fn execute(&self, spec: ShellExecSpec) -> ShellFuture<'_, Arc<dyn ShellExecution>> {
        Box::pin(async move {
            let execution = self.execute_spec(spec).await?;
            Ok(Arc::new(execution) as Arc<dyn ShellExecution>)
        })
    }
}

/// Shared by the execution module: lock a mutex, ignoring poisoning.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `Error.prototype.toString` of a thrown value, as `String(error)` produces it.
pub(crate) fn js_error_string(error: &SharedError) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let inner = error.get_ref();
    let name = hoh_error::HarnessError::of(inner).map_or("Error", |harness| harness.name());
    let mut message = String::new();
    if write!(message, "{inner}").is_err() {
        return "unprintable provider failure".to_owned();
    }
    if message.is_empty() {
        text.push_str(name);
    } else {
        let _ = write!(text, "{name}: {message}");
    }
    text
}
