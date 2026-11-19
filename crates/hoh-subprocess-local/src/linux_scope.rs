//! User-systemd capability probes and authoritative scope quiescence.
use crate::{
    error,
    linux_scope_selector::*,
    lock,
    managed_owner::{BoundProcessOwner, RangeExit},
    quoted,
    runner_launch::*,
    runner_protocol::*,
};
use futures_util::FutureExt;
use hoh_error::{BoxError, SharedError};
use hoh_subprocess::SubprocessOutcome;
use hoh_util_values::{is_js_white_space, js_trim};
use indexmap::{IndexMap, IndexSet};
use std::{
    io::Read,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
pub struct SystemctlResult {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<SharedError>,
}
pub type RunSync =
    Arc<dyn Fn(&str, &[String], &IndexMap<String, String>) -> SystemctlResult + Send + Sync>;
pub type PollSleep =
    Arc<dyn Fn(Duration) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>;
pub type SystemctlQuery = Arc<
    dyn Fn(String, Vec<String>) -> futures_util::future::BoxFuture<'static, SystemctlResult>
        + Send
        + Sync,
>;
#[derive(Clone)]
pub struct LinuxScopeInternals {
    pub systemd_run: String,
    pub systemctl: String,
    pub runner_invocation: Vec<String>,
    pub unix_socket_table: PathBuf,
    pub run_sync: RunSync,
    pub sleep: PollSleep,
    pub systemctl_query: SystemctlQuery,
}
impl Default for LinuxScopeInternals {
    fn default() -> Self {
        Self {
            systemd_run: "systemd-run".into(),
            systemctl: "systemctl".into(),
            runner_invocation: spawn_runner_invocation(),
            unix_socket_table: "/proc/net/unix".into(),
            run_sync: Arc::new(run_command),
            sleep: Arc::new(|delay| tokio::time::sleep(delay).boxed()),
            systemctl_query: Arc::new(|file, args| query_systemctl(file, args).boxed()),
        }
    }
}
async fn read_query_stream(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    which: &str,
    bytes: Arc<Mutex<Vec<u8>>>,
) -> Result<(), BoxError> {
    use tokio::io::AsyncReadExt;
    let mut buffer = [0; 8192];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|error| crate::linux_execve::system_error(error, "read", None))?;
        if count == 0 {
            return Ok(());
        }
        let mut output = lock(&bytes);
        let remaining = 1_048_576usize.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            return Err(Box::new(hoh_subprocess::SubprocessForeignError::Coded {
                message: format!("{which} maxBuffer length exceeded"),
                code: serde_json::Value::String("ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into()),
            }));
        }
    }
}
/// Node execFile's separate asynchronous adapter, including its error-bearing nonzero exits.
pub async fn query_systemctl(file: String, args: Vec<String>) -> SystemctlResult {
    let mut command = tokio::process::Command::new(&file);
    command
        .args(&args)
        .env_clear()
        .envs(manager_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return SystemctlResult {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(SharedError::new(
                    hoh_subprocess::SubprocessSpawnRejection::new(error, &file),
                )),
            };
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out = Arc::new(Mutex::new(vec![]));
    let err = Arc::new(Mutex::new(vec![]));
    let mut forced = None;
    let status = {
        let out_bytes = out.clone();
        let err_bytes = err.clone();
        let operation = async {
            futures_util::future::try_join3(
                async { child.wait().await.map_err(Into::<BoxError>::into) },
                async {
                    if let Some(stream) = stdout {
                        read_query_stream(stream, "stdout", out_bytes).await?;
                    }
                    Ok::<_, BoxError>(())
                },
                async {
                    if let Some(stream) = stderr {
                        read_query_stream(stream, "stderr", err_bytes).await?;
                    }
                    Ok::<_, BoxError>(())
                },
            )
            .await
            .map(|(status, (), ())| status)
        };
        tokio::pin!(operation);
        tokio::select! {result=&mut operation=>Some(result),_=tokio::time::sleep(Duration::from_secs(5))=>None}
    };
    let status = match status {
        Some(Ok(status)) => Some(status),
        failure => {
            if let Some(Err(error)) = failure {
                forced = Some(SharedError::new(error));
            }
            if let Some(pid) = child.id() {
                crate::spawn::signal_direct_process(pid, rustix::process::Signal::TERM);
            }
            child.wait().await.ok()
        }
    };
    let stdout = String::from_utf8_lossy(&lock(&out)).into_owned();
    let stderr = String::from_utf8_lossy(&lock(&err)).into_owned();
    let status = if forced.is_some() {
        None
    } else {
        status.and_then(|status| status.code())
    };
    let failure = if let Some(error) = forced {
        Some(error)
    } else if status != Some(0) {
        Some(SharedError::new(
            hoh_subprocess::SubprocessForeignError::Coded {
                message: format!(
                    "Command failed: {file}{}\n{stderr}",
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", args.join(" "))
                    }
                ),
                code: status.map_or(serde_json::Value::Null, serde_json::Value::from),
            },
        ))
    } else {
        None
    };
    SystemctlResult {
        status,
        stdout,
        stderr,
        error: failure,
    }
}
pub fn run_command(file: &str, args: &[String], env: &IndexMap<String, String>) -> SystemctlResult {
    let mut command = std::process::Command::new(file);
    command
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return SystemctlResult {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(SharedError::new(
                    hoh_subprocess::SubprocessSpawnRejection::new(error, file),
                )),
            };
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut bytes = vec![];
        if let Some(mut stream) = stdout {
            let _ = stream.read_to_end(&mut bytes);
        }
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = vec![];
        if let Some(mut stream) = stderr {
            let _ = stream.read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut failure = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Err(error) => {
                failure = Some(SharedError::new(error));
                break None;
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                    }
                    let status = child.wait().ok();
                    failure = Some(SharedError::new(
                        hoh_subprocess::SubprocessForeignError::Coded {
                            message: format!("spawnSync {file} ETIMEDOUT"),
                            code: serde_json::Value::String("ETIMEDOUT".into()),
                        },
                    ));
                    break status;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    SystemctlResult {
        status: status.and_then(|s| s.code()),
        stdout: String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned(),
        error: failure,
    }
}
pub fn manager_environment() -> IndexMap<String, String> {
    let mut env = child_env(None);
    env.insert("LC_ALL".into(), "C".into());
    env.shift_remove("SYSTEMD_LOG_TARGET");
    env
}
fn quiet_environment() -> IndexMap<String, String> {
    let mut env = manager_environment();
    env.insert("SYSTEMD_LOG_TARGET".into(), "null".into());
    env
}
fn stem(prefix: &str) -> Result<String, BoxError> {
    let bytes = crate::shim::random_bytes::<6>()?;
    let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{prefix}-{}-{suffix}", std::process::id()))
}
pub fn probe_linux_bootstrap(internals: &LinuxScopeInternals) -> bool {
    runner_invocation_available(&internals.runner_invocation)
        && runner_invocation_available(&spawn_runner_invocation())
}
pub fn probe_linux_scope(internals: &LinuxScopeInternals) -> Result<Option<Vec<String>>, BoxError> {
    let attempt = |literal: &[String]| -> Result<SystemctlResult, BoxError> {
        let unit = stem("dsh-subprocess-probe")?;
        let mut args = vec![
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            "--collect".into(),
        ];
        args.extend_from_slice(literal);
        args.extend([
            format!("--unit={unit}"),
            "--".into(),
            internals.systemctl.clone(),
            "--user".into(),
            "show".into(),
            format!("{unit}.scope"),
            "--property=ActiveState".into(),
            "--value".into(),
        ]);
        Ok((internals.run_sync)(
            &internals.systemd_run,
            &args,
            &quiet_environment(),
        ))
    };
    let literal = vec!["--expand-environment=no".into()];
    let current = attempt(&literal)?;
    if current.error.is_none() && current.status == Some(0) {
        return Ok(Some(literal));
    }
    let rejection = current
        .stderr
        .find("unrecognized option")
        .is_some_and(|start| current.stderr[start..].contains("expand-environment"));
    if !rejection {
        return Ok(None);
    }
    let old = attempt(&[])?;
    Ok((old.error.is_none() && old.status == Some(0)).then(Vec::new))
}
pub fn probe_linux_native(
    internals: &LinuxScopeInternals,
) -> Result<Option<Vec<String>>, BoxError> {
    if probe_linux_bootstrap(internals) {
        probe_linux_scope(internals)
    } else {
        Ok(None)
    }
}
pub fn probe_linux_manager(internals: &LinuxScopeInternals) -> bool {
    let result = (internals.run_sync)(
        &internals.systemctl,
        &[
            "--user".into(),
            "show".into(),
            "--property=Version".into(),
            "--value".into(),
        ],
        &manager_environment(),
    );
    result.error.is_none() && result.status == Some(0)
}
pub fn unix_socket_listener(path: &str, table: &std::path::Path) -> Option<String> {
    let text = crate::read_utf8(table).ok()?;
    for line in text.lines() {
        if !line.ends_with(&format!(" {path}")) {
            continue;
        }
        let fields = js_trim(line)
            .split(is_js_white_space)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if fields.get(7..).is_some_and(|parts| parts.join(" ") == path)
            && fields
                .get(3)
                .and_then(|s| u64::from_str_radix(s, 16).ok())
                .is_some_and(|flags| flags & 0x10000 != 0)
        {
            return fields.get(6).map(|s| s.to_string());
        }
    }
    None
}
pub fn linux_manager_listener(internals: &LinuxScopeInternals) -> Option<String> {
    let env = child_env(None);
    let runtime = env.get("XDG_RUNTIME_DIR").filter(|s| !s.is_empty())?;
    unix_socket_listener(
        &hoh_util_workspace_path::node_path::join(&[runtime, "systemd", "private"]),
        &internals.unix_socket_table,
    )
}
pub fn confirm_linux_manager(internals: &LinuxScopeInternals) -> bool {
    linux_manager_listener(internals).is_some() || probe_linux_manager(internals)
}
pub fn systemd_run_identity(internals: &LinuxScopeInternals) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let env = child_env(None);
    let candidates = if internals.systemd_run.contains('/') {
        vec![internals.systemd_run.clone()]
    } else {
        env.get("PATH")
            .map_or("", String::as_str)
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| hoh_util_workspace_path::node_path::join(&[s, &internals.systemd_run]))
            .collect()
    };
    for candidate in candidates {
        if let Ok(stat) = std::fs::metadata(candidate)
            && stat.is_file()
        {
            return Some(launcher_identity_text(
                stat.dev(),
                stat.ino(),
                stat.size(),
                stat.mtime() as f64 * 1000.0 + stat.mtime_nsec() as f64 / 1e6,
            ));
        }
    }
    None
}
fn launcher_identity_text(device: u64, inode: u64, size: u64, modified_ms: f64) -> String {
    [device as f64, inode as f64, size as f64, modified_ms]
        .map(hoh_util_values::js_number_string)
        .join(":")
}
#[cfg(test)]
mod identity_tests {
    #[test]
    fn formats_stat_numbers_as_node_numbers_in_the_launcher_cache_key() {
        assert_eq!(
            super::launcher_identity_text(1, 9_007_199_254_740_993, 4, 0.0),
            "1:9007199254740992:4:0"
        );
    }
}
impl LinuxScopeObservations for LinuxScopeInternals {
    fn probe(&self) -> Result<Option<Vec<String>>, BoxError> {
        probe_linux_native(self)
    }
    fn manager_listener(&self) -> Option<String> {
        linux_manager_listener(self)
    }
    fn confirm_manager(&self) -> bool {
        confirm_linux_manager(self)
    }
    fn launcher_identity(&self) -> Option<String> {
        systemd_run_identity(self)
    }
}
#[derive(Debug, thiserror::Error)]
pub enum LinuxScopeNotStartedError {
    #[error("{0}")]
    NotStarted(String),
}
#[derive(Clone)]
pub struct LinuxScopeStartup {
    pub files: LinuxLaunchFiles,
    pub kind: LinuxScopeKind,
    pub termination_signals: Arc<Mutex<IndexSet<String>>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxScopeKind {
    Subprocess,
    Terminal,
}
impl std::fmt::Display for LinuxScopeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Subprocess => "subprocess",
            Self::Terminal => "terminal",
        })
    }
}
impl LinuxScopeStartup {
    pub fn new(files: LinuxLaunchFiles, kind: LinuxScopeKind) -> Self {
        Self {
            files,
            kind,
            termination_signals: Arc::default(),
        }
    }
    pub fn resolve_outcome(
        &self,
        outcome: SubprocessOutcome,
    ) -> Result<SubprocessOutcome, BoxError> {
        if let Some(LinuxStartupError::Error { error }) =
            read_linux_startup_error(&self.files.startup_error_path)?
        {
            return Err(crate::shim::restore_spawn_error(error));
        }
        if self.files.request_path.exists()
            && !outcome
                .signal
                .as_ref()
                .is_some_and(|signal| lock(&self.termination_signals).contains(signal))
        {
            return Err(Box::new(LinuxScopeNotStartedError::NotStarted(format!(
                "{} scope exited before its bootstrap consumed the launch request; the provider probes native containment again before its next command",
                self.kind
            ))));
        }
        Ok(outcome)
    }
}
#[derive(Clone)]
pub struct DirectRange {
    pub running: Arc<AtomicBool>,
    pub signal: Arc<dyn Fn(rustix::process::Signal) -> bool + Send + Sync>,
    pub settled: RangeExit,
}
struct OwnerState {
    established: bool,
    stopped: bool,
    termination_requested: bool,
    kill_failure: Option<SharedError>,
    join_direct: bool,
    generation: u64,
    observation: Option<RangeExit>,
}
struct ScopeInner {
    unit: String,
    startup: LinuxScopeStartup,
    direct: DirectRange,
    internals: LinuxScopeInternals,
    state: Mutex<OwnerState>,
    wake: tokio::sync::Notify,
}
#[derive(Clone)]
pub struct SystemdScopeOwner(Arc<ScopeInner>);
#[derive(Debug, PartialEq, Eq)]
pub struct UnitState {
    pub load_state: String,
    pub active_state: String,
    pub tasks_current: Option<u64>,
}
pub fn parse_unit_state(unit: &str, text: &str) -> Result<UnitState, BoxError> {
    let mut values = IndexMap::new();
    for line in text.split('\n').map(|s| s.strip_suffix('\r').unwrap_or(s)) {
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(crate::runner_protocol::invalid_state(
                unit,
                "malformed state",
                text,
            ));
        };
        if key.is_empty() {
            return Err(crate::runner_protocol::invalid_state(
                unit,
                "malformed state",
                text,
            ));
        }
        if values.insert(key, value).is_some() {
            return Err(error(format!(
                "systemctl returned duplicate {key} for {unit}"
            )));
        }
    }
    let (Some(load), Some(active)) = (values.get("LoadState"), values.get("ActiveState")) else {
        return Err(crate::runner_protocol::invalid_state(
            unit,
            "incomplete state",
            text,
        ));
    };
    if values.len()
        != if values.contains_key("TasksCurrent") {
            3
        } else {
            2
        }
    {
        return Err(crate::runner_protocol::invalid_state(
            unit,
            "incomplete state",
            text,
        ));
    }
    let tasks = match values.get("TasksCurrent") {
        None | Some(&"[not set]") => None,
        Some(text) => {
            if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
                return Err(error(format!(
                    "systemctl returned a non-numeric TasksCurrent for {unit}: {}",
                    quoted(text)
                )));
            }
            text.parse().ok()
        }
    };
    Ok(UnitState {
        load_state: (*load).into(),
        active_state: (*active).into(),
        tasks_current: tasks,
    })
}
fn missing_unit(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.match_indices("unit").any(|(at, _)| {
            let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
            !line[..at].chars().next_back().is_some_and(word)
                && !line[at + 4..].chars().next().is_some_and(word)
                && ["could not be found", "not found", "not loaded"]
                    .iter()
                    .any(|needle| line[at + 4..].contains(needle))
        })
    })
}

/// Prepared native PTY invocation and ownership, sharing the ordinary bootstrap protocol.
pub struct LinuxTerminalScopeLaunch {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: IndexMap<String, String>,
    pub startup: LinuxScopeStartup,
    pub unit: String,
    pub internals: LinuxScopeInternals,
}
impl LinuxTerminalScopeLaunch {
    pub fn bind_owner(&self, direct: DirectRange) -> Arc<dyn BoundProcessOwner> {
        Arc::new(SystemdScopeOwner::new(
            self.unit.clone(),
            self.startup.clone(),
            direct,
            self.internals.clone(),
        ))
    }
    pub fn resolve_outcome(
        &self,
        outcome: SubprocessOutcome,
    ) -> Result<SubprocessOutcome, BoxError> {
        self.startup.resolve_outcome(outcome)
    }
    pub fn cleanup(&self) {
        cleanup_linux_launch_files(&self.startup.files);
    }
}
pub fn prepare_linux_terminal_scope(
    spec: &hoh_subprocess::SubprocessTerminalSpawnSpec,
    target_env: IndexMap<String, String>,
    literal_argv: &[String],
    internals: LinuxScopeInternals,
) -> Result<LinuxTerminalScopeLaunch, BoxError> {
    let files = create_linux_launch_files(&LinuxLaunchRequest {
        cwd: spec.cwd.clone(),
        env: target_env,
        pipe_fds: None,
        control: None,
        control_path: None,
    })?;
    let stem = unit_stem("dsh-terminal")?;
    let mut env = runner_environment(
        &files.request_path.to_string_lossy(),
        Some(&internals.runner_invocation),
    );
    // portable-pty already creates the launcher's session. Its target must not
    // detach from that controlling-terminal session when systemd-run forks it.
    env.insert(
        crate::runner_launch::BOOTSTRAP_SESSION_ENV.into(),
        "inherit".into(),
    );
    Ok(LinuxTerminalScopeLaunch {
        command: internals.systemd_run.clone(),
        args: scope_args(
            &stem,
            literal_argv,
            &internals.runner_invocation,
            &spec.argv,
        ),
        cwd: std::env::current_dir()?.to_string_lossy().into_owned(),
        env,
        startup: LinuxScopeStartup::new(files, LinuxScopeKind::Terminal),
        unit: format!("{stem}.scope"),
        internals,
    })
}
impl SystemdScopeOwner {
    pub fn new(
        unit: String,
        startup: LinuxScopeStartup,
        direct: DirectRange,
        internals: LinuxScopeInternals,
    ) -> Self {
        Self(Arc::new(ScopeInner {
            unit,
            startup,
            direct,
            internals,
            state: Mutex::new(OwnerState {
                established: false,
                stopped: false,
                termination_requested: false,
                kill_failure: None,
                join_direct: false,
                generation: 0,
                observation: None,
            }),
            wake: tokio::sync::Notify::new(),
        }))
    }
    fn observe_consumption(&self, state: &mut OwnerState) {
        if !self.0.startup.files.request_path.exists() {
            state.established = true;
        }
    }
    fn query(&self, args: &[String]) -> SystemctlResult {
        (self.0.internals.run_sync)(&self.0.internals.systemctl, args, &manager_environment())
    }
    fn absent(&self) -> Result<bool, SharedError> {
        let mut state = lock(&self.0.state);
        self.observe_consumption(&mut state);
        if state.established
            || (!self.0.direct.running.load(Ordering::Acquire)
                && self.0.startup.files.request_path.exists())
        {
            return Ok(false);
        }
        if let Some(error) = &state.kill_failure {
            return Err(error.clone());
        }
        Ok(true)
    }
    async fn active(&self) -> Result<bool, SharedError> {
        loop {
            let generation = {
                let mut state = lock(&self.0.state);
                self.observe_consumption(&mut state);
                state.generation
            };
            let running = self.0.direct.running.load(Ordering::Acquire);
            let result = (self.0.internals.systemctl_query)(
                self.0.internals.systemctl.clone(),
                vec![
                    "--user".into(),
                    "show".into(),
                    self.0.unit.clone(),
                    "--property=LoadState".into(),
                    "--property=ActiveState".into(),
                    "--property=TasksCurrent".into(),
                ],
            )
            .await;
            if lock(&self.0.state).generation != generation {
                return Ok(true);
            }
            let output = format!("{}\n{}", result.stdout, result.stderr);
            if result.status == Some(0) {
                let unit =
                    parse_unit_state(&self.0.unit, &result.stdout).map_err(SharedError::new)?;
                if unit.load_state == "not-found" && unit.active_state == "inactive" {
                    return self.absent();
                }
                if unit.load_state != "loaded" {
                    return Err(SharedError::new(error(format!(
                        "systemctl returned unknown state for {}: {}",
                        self.0.unit,
                        hoh_util_values::js_json_stringify(
                            &serde_json::json!({"loadState":unit.load_state,"activeState":unit.active_state})
                        )
                    ))));
                }
                {
                    lock(&self.0.state).established = true;
                }
                if unit.active_state == "inactive" || unit.active_state == "failed" {
                    return Ok(false);
                }
                if !["active", "activating", "reloading", "deactivating"]
                    .contains(&unit.active_state.as_str())
                {
                    return Err(SharedError::new(error(format!(
                        "systemctl returned unknown ActiveState for {}: {}",
                        self.0.unit,
                        quoted(&unit.active_state)
                    ))));
                }
                let empty = {
                    let state = lock(&self.0.state);
                    state.termination_requested
                        && unit.tasks_current == Some(0)
                        && (!self.0.direct.running.load(Ordering::Acquire)
                            || !self.0.startup.files.request_path.exists())
                };
                if empty {
                    let owner = self.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        owner.query(&["--user".into(), "stop".into(), owner.0.unit.clone()])
                    })
                    .await;
                    return Ok(false);
                }
                let (failure, join) = {
                    let mut state = lock(&self.0.state);
                    (
                        state.kill_failure.clone(),
                        running && std::mem::take(&mut state.join_direct),
                    )
                };
                if let Some(error) = failure {
                    if join {
                        let _ = self.0.direct.settled.clone().await;
                        continue;
                    }
                    return Err(error);
                }
                return Ok(true);
            }
            if missing_unit(&output) {
                return self.absent();
            }
            return Err(result.error.unwrap_or_else(|| {
                SharedError::new(error(format!(
                    "systemctl could not read {}: {}",
                    self.0.unit,
                    if js_trim(&output).is_empty() {
                        format!(
                            "exit {}",
                            result.status.map_or("null".into(), |v| v.to_string())
                        )
                    } else {
                        js_trim(&output).into()
                    }
                )))
            }));
        }
    }
}
impl BoundProcessOwner for SystemdScopeOwner {
    fn signal(&self, signal: rustix::process::Signal) {
        let pending = {
            let mut state = lock(&self.0.state);
            if state.stopped {
                return;
            }
            state.termination_requested = true;
            if self.0.direct.running.load(Ordering::Acquire) {
                lock(&self.0.startup.termination_signals).insert(
                    crate::spawn::signal_name(signal.as_raw())
                        .unwrap_or("SIGKILL")
                        .into(),
                );
            }
            self.observe_consumption(&mut state);
            !state.established
        };
        let mut sent = false;
        if pending && self.0.direct.running.load(Ordering::Acquire) {
            sent = (self.0.direct.signal)(signal);
        }
        let result = self.query(&[
            "--user".into(),
            "kill".into(),
            "--kill-who=all".into(),
            format!(
                "--signal={}",
                crate::spawn::signal_name(signal.as_raw()).unwrap_or("SIGKILL")
            ),
            self.0.unit.clone(),
        ]);
        {
            lock(&self.0.state).generation += 1;
        }
        self.0.wake.notify_waiters();
        if result.error.is_none() && result.status == Some(0) {
            if signal == rustix::process::Signal::KILL {
                let mut state = lock(&self.0.state);
                state.kill_failure = None;
                state.join_direct = false;
            }
            return;
        }
        if !pending && self.0.direct.running.load(Ordering::Acquire) {
            sent = (self.0.direct.signal)(signal);
        }
        if signal == rustix::process::Signal::KILL {
            let output = format!("{}\n{}", result.stdout, result.stderr);
            if !missing_unit(&output) {
                let mut state = lock(&self.0.state);
                state.kill_failure = Some(result.error.unwrap_or_else(|| {
                    SharedError::new(error(format!(
                        "systemctl could not signal {}: {}",
                        self.0.unit,
                        if js_trim(&output).is_empty() {
                            format!(
                                "exit {}",
                                result.status.map_or("null".into(), |v| v.to_string())
                            )
                        } else {
                            js_trim(&output).into()
                        }
                    )))
                }));
                state.join_direct = sent;
            }
        }
    }
    fn wait_for_exit(&self) -> RangeExit {
        let mut state = lock(&self.0.state);
        if state.stopped {
            return futures_util::future::ready(Ok(())).boxed().shared();
        }
        if let Some(future) = &state.observation {
            return future.clone();
        }
        let owner = self.clone();
        let future=async move{
            let mut interval=50;
            loop{
                let generation=lock(&owner.0.state).generation;
                match owner.active().await{
                    Ok(false)=>{lock(&owner.0.state).stopped=true;return Ok(());},
                    Err(error)=>{lock(&owner.0.state).observation=None;return Err(error);},
                    Ok(true)=>{
                        let wake=owner.0.wake.notified();tokio::pin!(wake);wake.as_mut().enable();
                        let unchanged=lock(&owner.0.state).generation==generation;
                        if unchanged{tokio::select!{_=&mut wake=>{},_=(owner.0.internals.sleep)(Duration::from_millis(interval))=>{}}}
                        if lock(&owner.0.state).established{interval=(interval*2).min(5000);}
                    }
                }
            }
        }.boxed().shared();
        state.observation = Some(future.clone());
        future
    }
    fn inspect_task_count(&self) -> Result<Option<u64>, BoxError> {
        let result = self.query(&[
            "--user".into(),
            "show".into(),
            "--property=LoadState".into(),
            "--property=ActiveState".into(),
            "--property=TasksCurrent".into(),
            self.0.unit.clone(),
        ]);
        if result.error.is_some() || result.status != Some(0) {
            return Ok(None);
        }
        let state = parse_unit_state(&self.0.unit, &result.stdout)?;
        Ok(
            (state.load_state == "loaded" && state.active_state == "active")
                .then_some(state.tasks_current)
                .flatten(),
        )
    }
    fn terminate_for_host_exit(&self) {
        if lock(&self.0.state).stopped {
            return;
        }
        if self.0.direct.running.load(Ordering::Acquire) {
            (self.0.direct.signal)(rustix::process::Signal::KILL);
        }
        let _ = self.query(&[
            "--user".into(),
            "kill".into(),
            "--kill-who=all".into(),
            "--signal=SIGKILL".into(),
            self.0.unit.clone(),
        ]);
    }
    fn cleanup(&self) -> Result<(), BoxError> {
        cleanup_linux_launch_files(&self.0.startup.files);
        Ok(())
    }
}
pub fn scope_args(
    unit: &str,
    literal: &[String],
    invocation: &[String],
    argv: &[String],
) -> Vec<String> {
    let mut args = vec![
        "--user".into(),
        "--scope".into(),
        "--quiet".into(),
        "--collect".into(),
    ];
    args.extend_from_slice(literal);
    args.extend([format!("--unit={unit}"), "--".into()]);
    args.extend_from_slice(invocation);
    args.push("--".into());
    args.extend_from_slice(argv);
    args
}
pub fn unit_stem(prefix: &str) -> Result<String, BoxError> {
    stem(prefix)
}
