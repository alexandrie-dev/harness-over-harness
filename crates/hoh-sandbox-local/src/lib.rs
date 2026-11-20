//! Linux local process-sandbox provider (`packages/sandbox/sandbox-local`).

pub mod profiles;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, plugin};
use hoh_node_addon_system::{
    LAUNCHER_BIN, LAUNCHER_FAILURE_EXIT, LandlockEnforcement, launcher_path, probe,
};
use hoh_sandbox::{
    ConfinedArgv, ConfinedSandboxMode, RunnerFailureRule, Sandbox, SandboxEnforcement,
    SandboxFuture, SandboxPolicy, SandboxProvider, SandboxUnavailableError, canonical_path,
};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use profiles::{bwrap_profile_args, landlock_profile_args};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub const DEFAULT_PROBE_TIMEOUT_MS: f64 = 5_000.0;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn default_probe_timeout_ms() -> f64 {
    DEFAULT_PROBE_TIMEOUT_MS
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub runner_command: Vec<String>,
    pub runner_failure_signatures: Vec<String>,
    #[serde(default = "default_probe_timeout_ms")]
    #[schemars(range(min = 0))]
    pub probe_timeout_ms: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            runner_command: Vec::new(),
            runner_failure_signatures: Vec::new(),
            probe_timeout_ms: DEFAULT_PROBE_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct SandboxLocalError(String);

fn local_error(message: impl Into<String>) -> BoxError {
    Box::new(SandboxLocalError(message.into()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn check_abort(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    signal
        .map(AbortSignal::throw_if_aborted)
        .transpose()
        .map(|_| ())
        .map_err(|reason| Box::new(reason) as BoxError)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalRunner {
    Bwrap,
    Landlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeVerdict {
    Full,
    Partial,
    Unusable,
}

impl ProbeVerdict {
    fn enforcement(self) -> Option<SandboxEnforcement> {
        match self {
            Self::Full => Some(SandboxEnforcement::Full),
            Self::Partial => Some(SandboxEnforcement::Partial),
            Self::Unusable => None,
        }
    }
}

impl From<LandlockEnforcement> for ProbeVerdict {
    fn from(value: LandlockEnforcement) -> Self {
        match value {
            LandlockEnforcement::Full => Self::Full,
            LandlockEnforcement::Partial => Self::Partial,
            LandlockEnforcement::Unusable => Self::Unusable,
        }
    }
}

pub type BwrapProbe = Arc<dyn Fn() -> bool + Send + Sync>;
pub type LandlockProbe = Arc<dyn Fn(&Path) -> ProbeVerdict + Send + Sync>;

#[derive(Clone, Default)]
pub struct SandboxInternals {
    pub platform: Option<String>,
    pub chain: Option<Vec<LocalRunner>>,
    pub probe_bwrap: Option<BwrapProbe>,
    pub probe_landlock: Option<LandlockProbe>,
    /// Test seam for the executable used by the real bwrap probe only.
    pub bwrap_executable: Option<PathBuf>,
    pub landlock_launcher: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct SelectedRunner {
    runner: LocalRunner,
    enforcement: SandboxEnforcement,
}

#[derive(Clone, Copy, Debug)]
enum Selection {
    Selected(SelectedRunner),
    Unavailable,
}

enum SelectionState {
    Empty,
    Running(watch::Receiver<Option<Selection>>),
    Ready(Selection),
}

struct SelectionCache {
    state: Mutex<SelectionState>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Default for SelectionCache {
    fn default() -> Self {
        Self {
            state: Mutex::new(SelectionState::Empty),
            task: Mutex::new(None),
        }
    }
}

impl Drop for SelectionCache {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

pub struct LocalSandboxProvider {
    runner_command: Option<Vec<String>>,
    configured_runner_failure_signatures: Vec<String>,
    probe_timeout: Duration,
    pub internals: Arc<Mutex<SandboxInternals>>,
    selection: Arc<SelectionCache>,
}

impl LocalSandboxProvider {
    pub fn new(config: Config) -> Result<Self, BoxError> {
        Self::new_with_internals(config, SandboxInternals::default())
    }

    pub fn new_with_internals(
        config: Config,
        internals: SandboxInternals,
    ) -> Result<Self, BoxError> {
        if config.runner_command.is_empty() && !config.runner_failure_signatures.is_empty() {
            return Err(local_error(
                "sandbox-local: runnerFailureSignatures requires runnerCommand",
            ));
        }
        if !config.runner_command.is_empty() && config.runner_failure_signatures.is_empty() {
            return Err(local_error(
                "sandbox-local: runnerCommand requires at least one runnerFailureSignatures entry",
            ));
        }
        if config
            .runner_failure_signatures
            .iter()
            .any(|signature| js_trim(signature).is_empty() || signature.contains(['\r', '\n']))
        {
            return Err(local_error(
                "sandbox-local: runnerFailureSignatures entries must be non-empty single-line strings",
            ));
        }
        let timeout = config.probe_timeout_ms;
        if !timeout.is_finite()
            || timeout <= 0.0
            || timeout.fract() != 0.0
            || timeout > MAX_SAFE_INTEGER
        {
            return Err(local_error(
                "sandbox-local: probeTimeoutMs must be a positive finite number",
            ));
        }
        Ok(Self {
            runner_command: (!config.runner_command.is_empty()).then_some(config.runner_command),
            configured_runner_failure_signatures: config.runner_failure_signatures,
            probe_timeout: Duration::from_millis(timeout as u64),
            internals: Arc::new(Mutex::new(internals)),
            selection: Arc::new(SelectionCache::default()),
        })
    }

    pub fn replace_internals(&self, internals: SandboxInternals) {
        *lock(&self.internals) = internals;
    }

    async fn select_runner(&self) -> Selection {
        let mut receiver = {
            let mut state = lock(&self.selection.state);
            match &*state {
                SelectionState::Ready(selection) => return *selection,
                SelectionState::Running(receiver) => receiver.clone(),
                SelectionState::Empty => {
                    let (sender, receiver) = watch::channel(None);
                    *state = SelectionState::Running(receiver.clone());
                    let cache = Arc::downgrade(&self.selection);
                    let internals = lock(&self.internals).clone();
                    let timeout = self.probe_timeout;
                    let task = tokio::spawn(async move {
                        let selection = Self::chain_verdict(internals, timeout).await;
                        if let Some(cache) = cache.upgrade() {
                            *lock(&cache.state) = SelectionState::Ready(selection);
                            lock(&cache.task).take();
                        }
                        sender.send_replace(Some(selection));
                    });
                    *lock(&self.selection.task) = Some(task.abort_handle());
                    receiver
                }
            }
        };
        loop {
            if let Some(selection) = *receiver.borrow() {
                return selection;
            }
            if receiver.changed().await.is_err() {
                // A failed selection task must fail closed and become cached.
                let selection = Selection::Unavailable;
                *lock(&self.selection.state) = SelectionState::Ready(selection);
                lock(&self.selection.task).take();
                return selection;
            }
        }
    }

    async fn select_protected_runner(&self) -> Selection {
        let internals = lock(&self.internals).clone();
        match Self::probe_runner(LocalRunner::Bwrap, &internals, self.probe_timeout)
            .await
            .enforcement()
        {
            Some(enforcement) => Selection::Selected(SelectedRunner {
                runner: LocalRunner::Bwrap,
                enforcement,
            }),
            None => Selection::Unavailable,
        }
    }

    async fn chain_verdict(internals: SandboxInternals, timeout: Duration) -> Selection {
        let chain = internals.chain.clone().unwrap_or_else(|| {
            let platform = internals
                .platform
                .as_deref()
                .unwrap_or(std::env::consts::OS);
            if platform == "linux" {
                vec![LocalRunner::Bwrap, LocalRunner::Landlock]
            } else {
                Vec::new()
            }
        });
        let Some(first) = chain.first().copied() else {
            return Selection::Unavailable;
        };
        if chain.len() == 1 {
            return Selection::Selected(SelectedRunner {
                runner: first,
                enforcement: SandboxEnforcement::Full,
            });
        }
        for runner in chain {
            let verdict = Self::probe_runner(runner, &internals, timeout).await;
            if let Some(enforcement) = verdict.enforcement() {
                return Selection::Selected(SelectedRunner {
                    runner,
                    enforcement,
                });
            }
        }
        Selection::Unavailable
    }

    async fn probe_runner(
        runner: LocalRunner,
        internals: &SandboxInternals,
        timeout: Duration,
    ) -> ProbeVerdict {
        match runner {
            LocalRunner::Bwrap => {
                let usable = match &internals.probe_bwrap {
                    Some(probe) => probe(),
                    None => {
                        default_probe_bwrap(
                            timeout,
                            internals
                                .bwrap_executable
                                .clone()
                                .unwrap_or_else(|| PathBuf::from("bwrap")),
                        )
                        .await
                    }
                };
                if usable {
                    ProbeVerdict::Full
                } else {
                    ProbeVerdict::Unusable
                }
            }
            LocalRunner::Landlock => {
                let Some(launcher) = resolve_landlock_launcher(internals) else {
                    return ProbeVerdict::Unusable;
                };
                if let Some(probe) = &internals.probe_landlock {
                    probe(&launcher)
                } else {
                    tokio::task::spawn_blocking(move || {
                        ProbeVerdict::from(probe(Some(&launcher), Some(timeout)))
                    })
                    .await
                    .unwrap_or(ProbeVerdict::Unusable)
                }
            }
        }
    }

    async fn runner_argv(
        &self,
        selected: SelectedRunner,
        policy: &SandboxPolicy,
    ) -> Result<Vec<String>, BoxError> {
        match selected.runner {
            LocalRunner::Bwrap => {
                let mut argv = vec!["bwrap".to_owned()];
                argv.extend(bwrap_profile_args(policy).await?);
                Ok(argv)
            }
            LocalRunner::Landlock => {
                let internals = lock(&self.internals).clone();
                let launcher = resolve_landlock_launcher(&internals).ok_or_else(|| {
                    local_error("sandbox-local: Landlock launcher path is unavailable")
                })?;
                let launcher = launcher.into_os_string().into_string().map_err(|_| {
                    local_error("sandbox-local: Landlock launcher path is not valid UTF-8")
                })?;
                let mut argv = vec![launcher];
                argv.extend(landlock_profile_args(policy).await?);
                Ok(argv)
            }
        }
    }
}

impl SandboxProvider for LocalSandboxProvider {
    fn confine<'a>(
        &'a self,
        argv: &'a [String],
        mut policy: SandboxPolicy,
        signal: Option<AbortSignal>,
    ) -> SandboxFuture<'a, ConfinedArgv> {
        Box::pin(async move {
            check_abort(signal.as_ref())?;
            policy.workspace_root = canonical_path(&policy.workspace_root).await;
            check_abort(signal.as_ref())?;
            if let Some(runner) = &self.runner_command {
                let mut wrapped = runner.clone();
                wrapped.extend(bwrap_profile_args(&policy).await?);
                check_abort(signal.as_ref())?;
                wrapped.push("--".into());
                wrapped.extend_from_slice(argv);
                return Ok(ConfinedArgv {
                    argv: wrapped,
                    enforcement: SandboxEnforcement::Full,
                    denial_signatures: vec![
                        "read-only file system".into(),
                        "permission denied".into(),
                    ],
                    runner_failure_rules: vec![RunnerFailureRule {
                        allowed_exit_codes: None,
                        fatal_signatures: self.configured_runner_failure_signatures.clone(),
                        informational_lines: None,
                    }],
                });
            }
            let selected = if policy
                .protected_roots
                .as_ref()
                .is_some_and(|roots| !roots.is_empty())
            {
                // Landlock is allow-only and cannot safely subtract a mutable same-user path from
                // a host-wide read grant without a path-open race. Protected roots therefore
                // require bwrap's mount-namespace mask or fail closed.
                self.select_protected_runner().await
            } else {
                self.select_runner().await
            };
            check_abort(signal.as_ref())?;
            let Selection::Selected(selected) = selected else {
                return Err(SandboxUnavailableError::new(policy.mode, None).into());
            };
            let mut wrapped = self.runner_argv(selected, &policy).await?;
            check_abort(signal.as_ref())?;
            wrapped.push("--".into());
            wrapped.extend_from_slice(argv);
            let (mut denial_signatures, runner_failure_rules) = runner_facts(selected.runner);
            if policy
                .protected_roots
                .as_ref()
                .is_some_and(|roots| !roots.is_empty())
                && !denial_signatures
                    .iter()
                    .any(|value| value == "permission denied")
            {
                denial_signatures.push("permission denied".into());
            }
            Ok(ConfinedArgv {
                argv: wrapped,
                enforcement: selected.enforcement,
                denial_signatures,
                runner_failure_rules,
            })
        })
    }
}

fn resolve_landlock_launcher(internals: &SandboxInternals) -> Option<PathBuf> {
    internals
        .landlock_launcher
        .clone()
        .or_else(|| launcher_path().ok())
}

async fn default_probe_bwrap(timeout: Duration, executable: PathBuf) -> bool {
    let policy = SandboxPolicy {
        mode: ConfinedSandboxMode::ReadOnly,
        workspace_root: "/".into(),
        additional_writable_roots: None,
        session_id: None,
        protected_roots: None,
        unrestricted_writes: false,
        ready_dir: None,
    };
    let Ok(profile) = bwrap_profile_args(&policy).await else {
        return false;
    };
    struct CancelOnDrop(Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    // Dropping JoinHandle cannot cancel a blocking task. This shared flag lets
    // disposal of the async owner ask that task to kill and reap its child.
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_on_drop = CancelOnDrop(cancelled.clone());
    let result = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command
            .args(profile)
            .arg("--")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if !cancelled.load(Ordering::Acquire) && started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    })
    .await
    .unwrap_or(false);
    drop(cancel_on_drop);
    result
}

fn runner_facts(runner: LocalRunner) -> (Vec<String>, Vec<RunnerFailureRule>) {
    match runner {
        LocalRunner::Bwrap => (
            vec!["read-only file system".into()],
            vec![RunnerFailureRule {
                allowed_exit_codes: None,
                fatal_signatures: vec!["bwrap: ".into()],
                informational_lines: None,
            }],
        ),
        LocalRunner::Landlock => (
            vec!["permission denied".into()],
            vec![RunnerFailureRule {
                allowed_exit_codes: Some(vec![i32::from(LAUNCHER_FAILURE_EXIT)]),
                fatal_signatures: vec![format!("{LAUNCHER_BIN}: ")],
                informational_lines: Some(vec![format!(
                    "{LAUNCHER_BIN}: partial enforcement (older Landlock ABI)"
                )]),
            }],
        ),
    }
}

#[plugin(LocalSandboxPlugin, name = "LocalSandboxProvider")]
pub async fn apply(ctx: Context, config: Cfg<Config>) -> Result<(), BoxError> {
    ctx.provide::<Sandbox>(Arc::new(LocalSandboxProvider::new(
        (*config.get()).clone(),
    )?))
    .await?;
    Ok(())
}
