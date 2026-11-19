//! Local Linux subprocess and PTY provider. Windows native bindings await stage 5.
pub mod control_spawn;
pub mod linux_execve;
pub mod linux_scope;
pub mod linux_scope_selector;
pub mod managed_owner;
pub mod output;
pub mod process_inspector;
pub mod runner_launch;
pub mod runner_protocol;
pub mod shell_activity;
mod shim;
pub mod spawn;
pub mod spawn_runner;
pub mod terminal;
#[cfg(test)]
mod tests;

use hoh_error::{BoxError, SharedError};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    TypeError(String),
    #[error("local subprocess teardown failed")]
    Teardown(Vec<SharedError>),
    #[error("terminal managed-range cleanup failed")]
    ManagedTeardown(Vec<SharedError>),
}
impl LocalError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error(_) => "Error",
            Self::TypeError(_) => "TypeError",
            Self::Teardown(_) | Self::ManagedTeardown(_) => "AggregateError",
        }
    }
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::TypeError(_) => Some("ERR_INVALID_ARG_VALUE"),
            _ => None,
        }
    }
}
/// Explicit primitive reasons that JavaScript AbortSignal can carry without an Error object.
#[derive(Debug, thiserror::Error)]
pub enum SubprocessAbortReason {
    #[error("{}",hoh_util_values::js_string(.0))]
    Value(serde_json::Value),
    #[error("undefined")]
    Undefined,
}
impl SubprocessAbortReason {
    pub fn nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Value(serde_json::Value::Null))
    }
}
pub(crate) fn error(message: impl Into<String>) -> BoxError {
    Box::new(LocalError::Error(message.into()))
}
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn quoted(text: &str) -> String {
    hoh_util_values::js_json_stringify(&serde_json::Value::String(text.into()))
}
pub(crate) fn read_utf8(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}
pub(crate) fn error_text(error: &(dyn std::error::Error + 'static)) -> String {
    let error = SharedError::of(error).map_or(error, |shared| shared.get_ref());
    if let Some(reason) = error.downcast_ref::<SubprocessAbortReason>() {
        return reason.to_string();
    }
    let name = error_name(error);
    let message = error.to_string();
    if message.is_empty() {
        name.into()
    } else if name.is_empty() {
        message
    } else {
        format!("{name}: {message}")
    }
}
pub(crate) fn error_name<'a>(error: &'a (dyn std::error::Error + 'static)) -> &'a str {
    let error = SharedError::of(error).map_or(error, |shared| shared.get_ref());
    hoh_error::HarnessError::of(error)
        .map(|e| e.name())
        .or_else(|| error.downcast_ref::<LocalError>().map(LocalError::name))
        .or_else(|| {
            error
                .is::<hoh_timeout::AbortError>()
                .then_some("AbortError")
        })
        .or_else(|| {
            error
                .is::<hoh_timeout::DomTimeoutError>()
                .then_some("TimeoutError")
        })
        .or_else(|| {
            error
                .is::<hoh_util_values::JsonParseError>()
                .then_some("SyntaxError")
        })
        .or_else(|| {
            error
                .is::<linux_scope::LinuxScopeNotStartedError>()
                .then_some("LinuxScopeNotStartedError")
        })
        .or_else(|| {
            error
                .downcast_ref::<runner_protocol::RunnerFailure>()
                .map(|e| e.diagnostic().name.as_str())
        })
        .unwrap_or("Error")
}

use futures_util::future::join_all;
use hoh_kernel::{Cfg, Context, NoConfig, Plugin, Undo};
use hoh_subprocess::*;
use indexmap::IndexMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct Managed {
    processes: Mutex<Vec<Arc<spawn::LocalProcessLifecycle>>>,
    terminals: Mutex<Vec<Arc<terminal::LocalTerminalHandle>>>,
    storage: Arc<output::SpillStorage>,
    controls: Mutex<Vec<std::sync::Weak<spawn::ControlGate>>>,
}
impl Managed {
    fn force_stop(&self) {
        for process in lock(&self.processes).iter() {
            shim::terminate_for_host_exit(|| process.terminate_for_host_exit());
        }
        for terminal in lock(&self.terminals).iter() {
            shim::terminate_for_host_exit(|| terminal.terminate_for_host_exit());
        }
    }
    async fn dispose(self: Arc<Self>) -> Result<(), BoxError> {
        let processes = lock(&self.processes).clone();
        let terminals = lock(&self.terminals).clone();
        for process in &processes {
            process.terminate();
        }
        let process_results = join_all(processes.iter().map(|process| async move {
            futures_util::future::try_join(
                async {
                    let _ = process.done.clone().await;
                    Ok::<_, BoxError>(())
                },
                async { process.wait_for_exit(None).await.map(|_| ()) },
            )
            .await
            .map(|_| ())
        }));
        let terminal_results = join_all(terminals.iter().map(|terminal| async move {
            terminal
                .terminate_shared()
                .await
                .map_err(Into::<BoxError>::into)
        }));
        let (process_results, terminal_results) =
            futures_util::future::join(process_results, terminal_results).await;
        for gate in lock(&self.controls)
            .drain(..)
            .filter_map(|gate| gate.upgrade())
        {
            gate.close();
        }
        let mut failures = process_results
            .into_iter()
            .chain(terminal_results)
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            self.force_stop();
        }
        match failures.len() {
            0 => Ok(()),
            1 => Err(failures.remove(0)),
            _ => Err(Box::new(LocalError::Teardown(
                failures.into_iter().map(SharedError::new).collect(),
            ))),
        }
    }
}

/// The local provider; mutable launch operations are explicit test seams rather than process globals.
pub struct LocalSubprocessRuntime {
    ctx: Context,
    managed: Arc<Managed>,
    pub internals: Mutex<spawn::SpawnInternals>,
    pub terminal_inspector: Mutex<Option<Arc<dyn process_inspector::ProcessInspector>>>,
    selector: Arc<Mutex<linux_scope_selector::LinuxScopeSelector>>,
    fallback_warning_issued: AtomicBool,
}
type SelectedContainment = (
    Option<(linux_scope::LinuxScopeInternals, Vec<String>)>,
    Option<u64>,
);
impl Plugin for LocalSubprocessRuntime {
    const NAME: &'static str = "LocalSubprocessRuntime";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let runtime = Arc::new(Self::new(ctx.clone())?);
        provide_subprocess(&ctx, runtime).await?;
        Ok(())
    }
}
impl LocalSubprocessRuntime {
    pub fn new(ctx: Context) -> Result<Self, BoxError> {
        Self::new_with_observations(ctx, linux_scope::LinuxScopeInternals::default())
    }
    pub fn new_with_observations(
        ctx: Context,
        observations: impl linux_scope_selector::LinuxScopeObservations + 'static,
    ) -> Result<Self, BoxError> {
        let managed = Arc::new(Managed::default());
        let teardown = managed.clone();
        ctx.effect("local subprocess teardown", move || {
            Undo::future(move || async move { teardown.dispose().await })
        })?;
        Ok(Self {
            ctx,
            managed,
            internals: Mutex::default(),
            terminal_inspector: Mutex::default(),
            selector: Arc::new(Mutex::new(linux_scope_selector::LinuxScopeSelector::new(
                observations,
            ))),
            fallback_warning_issued: AtomicBool::new(false),
        })
    }
    pub fn terminate_for_host_exit(&self) {
        self.managed.force_stop();
    }
    fn select_containment(&self) -> Result<SelectedContainment, BoxError> {
        use linux_scope_selector::LinuxScopeDecision;
        let decision = lock(&self.selector).select()?;
        let (kind, reason, probe) = match &decision {
            LinuxScopeDecision::Scope { reason, probe, .. } => ("scope", reason.to_string(), probe),
            LinuxScopeDecision::Fallback { reason, probe } => {
                ("fallback", reason.to_string(), probe)
            }
        };
        let probe = probe.as_ref().map(|probe| match probe {
            linux_scope_selector::LinuxProbeTrigger::First => "first",
            linux_scope_selector::LinuxProbeTrigger::ScopeNotStarted => "scope-not-started",
            linux_scope_selector::LinuxProbeTrigger::NewListener => "new-listener",
            linux_scope_selector::LinuxProbeTrigger::LauncherChanged => "launcher-changed",
        });
        self.ctx.logger().debug(format!(
            "linux native containment: {kind} ({reason}{})",
            probe.map_or(String::new(), |probe| format!(", probed after {probe}"))
        ));
        Ok(match decision {
            LinuxScopeDecision::Scope {
                literal_argv,
                generation,
                ..
            } => (
                Some((
                    lock(&self.internals)
                        .linux_scope
                        .as_ref()
                        .map(|scope| scope.0.clone())
                        .unwrap_or_default(),
                    literal_argv,
                )),
                Some(generation),
            ),
            LinuxScopeDecision::Fallback { .. } => {
                if !self.fallback_warning_issued.swap(true, Ordering::AcqRel) {
                    self.ctx.logger().warn("subprocess-local is using weaker process-tree containment because the current user-systemd scope or private bootstrap is unavailable; descendants that escape the process group or direct-parent tree are not guaranteed to terminate or delay waitForExit()");
                }
                (None, None)
            }
        })
    }
    fn watch_start(
        &self,
        done: SubprocessDone,
        generation: Option<u64>,
    ) -> Result<SubprocessDone, BoxError> {
        if let Some(generation) = generation {
            let selector = self.selector.clone();
            let watched = subprocess_done(async move {
                let result = done.await;
                if let Err(error) = &result
                    && error
                        .get_ref()
                        .is::<linux_scope::LinuxScopeNotStartedError>()
                {
                    lock(&selector).scope_not_started(generation);
                }
                result
            });
            let owned = watched.clone();
            self.ctx
                .spawn("subprocess native start", move |_cancel| async move {
                    let _ = owned.await;
                    Ok(())
                })?;
            return Ok(watched);
        }
        Ok(done)
    }
}
impl SubprocessRuntime for LocalSubprocessRuntime {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        env: Option<&'a IndexMap<String, String>>,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        Box::pin(async move {
            if command.is_empty() {
                return Err(error("subprocess-local: executable must be non-empty"));
            }
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            let absolute = command.starts_with('/');
            if !absolute && command.contains('/') {
                return Err(error(format!(
                    "subprocess-local: command {} is a relative path; use an absolute path or a bare PATH name",
                    quoted(command)
                )));
            }
            let overlay = env.map(|env| {
                env.iter()
                    .map(|(k, v)| (k.clone(), Some(v.clone())))
                    .collect()
            });
            let environment = runner_launch::child_env(overlay.as_ref());
            let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
            let candidates = if absolute {
                vec![command.to_owned()]
            } else {
                environment
                    .get("PATH")
                    .map_or("", String::as_str)
                    .split(':')
                    .map(|dir| hoh_util_workspace_path::node_path::resolve(&cwd, &[dir, command]))
                    .collect()
            };
            for candidate in candidates {
                if let Some(signal) = &signal {
                    signal.throw_if_aborted()?;
                }
                let target = candidate.clone();
                let usable = tokio::task::spawn_blocking(move || {
                    std::fs::metadata(&target).is_ok_and(|m| m.is_file())
                        && rustix::fs::access(target, rustix::fs::Access::EXEC_OK).is_ok()
                })
                .await?;
                if usable {
                    if let Some(signal) = &signal {
                        signal.throw_if_aborted()?;
                    }
                    return Ok(candidate);
                }
            }
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            Err(SubprocessExecutableNotFoundError::new(if absolute {
                format!(
                    "subprocess-local: command {} is not an executable file",
                    quoted(command)
                )
            } else {
                format!(
                    "subprocess-local: command {} was not found on PATH",
                    quoted(command)
                )
            })
            .into())
        })
    }
    fn terminal_environment(
        &self,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        Box::pin(async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            let default_shell = tokio::task::spawn_blocking(|| {
                std::env::var_os("SHELL")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string_lossy().into_owned())
                    .or_else(shim::user_info_shell)
            })
            .await?;
            Ok(SubprocessTerminalEnvironment {
                platform: SubprocessPlatform::Posix,
                default_shell,
            })
        })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        spawn::validate_subprocess_spec(&spec)?;
        runner_launch::target_environment(&spec)?;
        let (scope, generation) = self.select_containment()?;
        let mut internals = lock(&self.internals).clone();
        internals.linux_scope = scope;
        internals.spill_storage = Some(self.managed.storage.clone());
        internals.on_spill_failure = Some(output::log_spill_failure(
            self.ctx.logger(),
            "subprocess-local".into(),
        ));
        let mut local = spawn::spawn_subprocess(&self.ctx, spec, internals)?;
        if let Some(gate) = &local.control_gate {
            let mut controls = lock(&self.managed.controls);
            controls.retain(|gate| gate.strong_count() > 0);
            controls.push(Arc::downgrade(gate));
        }
        local.handle.done = self.watch_start(local.handle.done.clone(), generation)?;
        lock(&self.managed.processes).push(local.lifecycle.clone());
        let state = Arc::downgrade(&self.managed);
        let lifecycle = local.lifecycle.clone();
        self.ctx
            .spawn("subprocess release", move |_cancel| async move {
                let _ = lifecycle.done.clone().await;
                if lifecycle.wait_for_exit(None).await.is_ok()
                    && let Some(state) = state.upgrade()
                {
                    lock(&state.processes).retain(|existing| !Arc::ptr_eq(existing, &lifecycle));
                }
                Ok(())
            })?;
        Ok(local.handle)
    }
    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        Box::pin(async move {
            if spec.argv.first().is_none_or(String::is_empty) {
                return Err(SubprocessError::new(
                    "subprocess-local: terminal argv must contain a program",
                    SubprocessErrorCode::InvalidArgv,
                )
                .into());
            }
            if let Some(signal) = &spec.signal {
                signal.throw_if_aborted()?;
            }
            let (scope, generation) = self.select_containment()?;
            let inspector = lock(&self.terminal_inspector)
                .clone()
                .unwrap_or_else(process_inspector::create_process_inspector);
            let (mut handle, local) =
                terminal::spawn_terminal(&self.ctx, spec, inspector, scope).await?;
            handle.done = self.watch_start(handle.done.clone(), generation)?;
            lock(&self.managed.terminals).push(local.clone());
            let state = Arc::downgrade(&self.managed);
            let target = Arc::downgrade(&local);
            local.on_quiescence(Arc::new(move || {
                if let (Some(state), Some(target)) = (state.upgrade(), target.upgrade()) {
                    lock(&state.terminals).retain(|existing| !Arc::ptr_eq(existing, &target));
                }
            }));
            Ok(handle)
        })
    }
}
