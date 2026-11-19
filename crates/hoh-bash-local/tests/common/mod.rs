#![allow(dead_code)]

use futures_util::FutureExt;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Disposer, Service, Undo};
use hoh_shell::{
    Shell, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExpiryPolicy, ShellProcess,
    ShellRunResult,
};
use hoh_subprocess::{
    SubprocessCollectedOutputs, SubprocessDone, SubprocessEnvironment, SubprocessFuture,
    SubprocessHandle, SubprocessLifecycle, SubprocessOutcome, SubprocessOutputMode,
    SubprocessOutputRead, SubprocessOutputReader, SubprocessRuntime, SubprocessSpawnRejection,
    SubprocessSpawnSpec, SubprocessTerminalEnvironment, SubprocessTerminalHandle,
    SubprocessTerminalSpawnSpec, provide_subprocess, scrubbed_parent_env, subprocess_done,
};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    task::JoinHandle,
};

pub struct FixtureError;
impl std::fmt::Debug for FixtureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FixtureError")
    }
}
impl std::fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("test provider does not open terminals")
    }
}
impl std::error::Error for FixtureError {}

pub struct BufferReader {
    state: Mutex<BufferState>,
    directory: PathBuf,
    name: &'static str,
    cap: u64,
}
struct BufferState {
    bytes: Vec<u8>,
    spill_path: Option<String>,
}
impl BufferReader {
    pub fn new(cap: u64, directory: &Path, name: &'static str) -> Self {
        Self {
            state: Mutex::new(BufferState {
                bytes: Vec::new(),
                spill_path: None,
            }),
            directory: directory.to_owned(),
            name,
            cap,
        }
    }
    fn append(&self, chunk: &[u8]) {
        let mut state = self.state.lock().unwrap();
        state.bytes.extend_from_slice(chunk);
        if state.bytes.len() as u64 > self.cap && state.spill_path.is_none() {
            let path = self.directory.join(format!(
                "{}-{}",
                self.name,
                hoh_bash_local::ENV_OVERRIDES.len()
            ));
            let unique = path.with_extension(format!("{}", std::process::id()));
            let _ = std::fs::write(&unique, &state.bytes);
            state.spill_path = Some(unique.to_string_lossy().into_owned());
        }
    }
}
impl SubprocessOutputReader for BufferReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let state = self.state.lock().unwrap();
        let total = state.bytes.len() as u64;
        let kept = total.min(self.cap);
        let base = total - kept;
        let lossy = from_byte < base;
        let start = from_byte.max(base);
        let tail_start = (state.bytes.len() as u64 - kept) as usize;
        let relative = (start - base) as usize;
        let bytes = &state.bytes[tail_start + relative..];
        SubprocessOutputRead {
            text: String::from_utf8_lossy(bytes).into_owned(),
            next_offset: total,
            lossy,
            spill_path: state.spill_path.clone(),
        }
    }
}

pub struct LocalLifecycle {
    pid: u32,
    grace_ms: f64,
    settled: Arc<AtomicBool>,
    terminating: AtomicBool,
    done: Mutex<Option<SubprocessDone>>,
}
impl LocalLifecycle {
    fn signal(&self, signal: rustix::process::Signal) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid as i32) {
            let _ = rustix::process::kill_process_group(pid, signal);
        }
    }
    pub fn is_settled(&self) -> bool {
        self.settled.load(Ordering::SeqCst)
    }
}
impl SubprocessLifecycle for LocalLifecycle {
    fn terminate(&self) {
        if self.settled.load(Ordering::SeqCst) || self.terminating.swap(true, Ordering::SeqCst) {
            return;
        }
        self.signal(rustix::process::Signal::TERM);
        let settled = self.settled.clone();
        let pid = self.pid;
        let grace = self.grace_ms;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(grace.max(1.0) as u64)).await;
            if !settled.load(Ordering::SeqCst)
                && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        });
    }

    fn wait_for_exit(&self, _signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        let done = self.done.lock().unwrap().clone();
        async move {
            if let Some(done) = done {
                let _ = done.await;
            }
            Ok(true)
        }
        .boxed()
    }
}

pub struct LocalRuntime {
    directory: TempDir,
    processes: Arc<Mutex<Vec<Arc<LocalLifecycle>>>>,
}
impl LocalRuntime {
    pub fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
            processes: Arc::default(),
        }
    }
    pub async fn shutdown(&self) {
        let processes = self.processes.lock().unwrap().clone();
        for process in processes.iter().filter(|process| !process.is_settled()) {
            process.terminate();
        }
        for process in processes {
            let _ = process.wait_for_exit(None).await;
        }
    }

    pub async fn provide(self: &Arc<Self>, ctx: &Context) -> Result<Disposer, BoxError> {
        let provision = provide_subprocess(ctx, self.clone()).await?;
        let runtime = self.clone();
        Ok(ctx.effect("test local subprocess teardown", move || {
            Undo::future(move || async move {
                runtime.shutdown().await;
                provision.dispose().await;
                Ok(())
            })
        })?)
    }
}

fn collect_spec(mode: &SubprocessOutputMode) -> (u64, bool) {
    match mode {
        SubprocessOutputMode::Collect(collect) => (collect.max_bytes, true),
        _ => (u64::MAX, false),
    }
}

fn reader_task(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    output: Arc<BufferReader>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => output.append(&buffer[..count]),
            }
        }
    })
}

fn signal_name(status: std::process::ExitStatus) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|signal| match signal {
            1 => "SIGHUP".to_owned(),
            2 => "SIGINT".to_owned(),
            9 => "SIGKILL".to_owned(),
            15 => "SIGTERM".to_owned(),
            other => format!("SIG{other}"),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

impl SubprocessRuntime for LocalRuntime {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        _env: Option<&'a IndexMap<String, String>>,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        async move { Ok(command.to_owned()) }.boxed()
    }

    fn terminal_environment(
        &self,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        async move { Err(Box::new(FixtureError) as BoxError) }.boxed()
    }

    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        let program = spec.argv.first().cloned().unwrap_or_default();
        let mut command = Command::new(&program);
        command.args(spec.argv.iter().skip(1));
        command.current_dir(&spec.cwd);
        command.env_clear();
        let mut environment: SubprocessEnvironment = scrubbed_parent_env(None)
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();
        for (name, value) in spec.env.iter().flatten() {
            match value {
                Some(value) => {
                    environment.insert(name.clone(), Some(value.clone()));
                }
                None => {
                    environment.shift_remove(name);
                }
            }
        }
        command.envs(
            environment
                .iter()
                .filter_map(|(name, value)| value.as_ref().map(|value| (name, value))),
        );
        command.stdin(
            if matches!(
                spec.stdio.stdin,
                hoh_subprocess::SubprocessStdinMode::Data { .. }
            ) {
                Stdio::piped()
            } else {
                Stdio::null()
            },
        );
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            Box::new(SubprocessSpawnRejection::new(error, &program)) as BoxError
        })?;
        let pid = child.id().unwrap_or_default();
        let (stdout_cap, _) = collect_spec(&spec.stdio.stdout);
        let (stderr_cap, _) = collect_spec(&spec.stdio.stderr);
        let stdout = Arc::new(BufferReader::new(
            stdout_cap,
            self.directory.path(),
            "stdout",
        ));
        let stderr = Arc::new(BufferReader::new(
            stderr_cap,
            self.directory.path(),
            "stderr",
        ));
        let stdout_task = reader_task(child.stdout.take().unwrap(), stdout.clone());
        let stderr_task = reader_task(child.stderr.take().unwrap(), stderr.clone());
        if let hoh_subprocess::SubprocessStdinMode::Data { data } = spec.stdio.stdin {
            let mut stdin = child.stdin.take().unwrap();
            tokio::spawn(async move {
                let _ = stdin.write_all(data.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
        let settled = Arc::new(AtomicBool::new(false));
        let lifecycle = Arc::new(LocalLifecycle {
            pid,
            grace_ms: spec.grace_ms,
            settled: settled.clone(),
            terminating: AtomicBool::new(false),
            done: Mutex::new(None),
        });
        self.processes.lock().unwrap().push(lifecycle.clone());
        let done = wait_child(child, stdout_task, stderr_task, settled);
        *lifecycle.done.lock().unwrap() = Some(done.clone());
        if let Some(signal) = spec.signal {
            let lifecycle = lifecycle.clone();
            let mut done = done.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = &mut done => {}
                    _ = signal.cancelled() => lifecycle.terminate(),
                }
            });
        }
        Ok(SubprocessHandle {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            collected: SubprocessCollectedOutputs {
                stdout: Some(stdout),
                stderr: Some(stderr),
            },
            done,
            lifecycle,
        })
    }

    fn spawn_terminal(
        &self,
        _spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        async move { Err(Box::new(FixtureError) as BoxError) }.boxed()
    }
}

fn wait_child(
    mut child: Child,
    stdout: JoinHandle<()>,
    stderr: JoinHandle<()>,
    settled: Arc<AtomicBool>,
) -> SubprocessDone {
    subprocess_done(async move {
        let status = child.wait().await.map_err(SharedError::new)?;
        let _ = stdout.await;
        let _ = stderr.await;
        settled.store(true, Ordering::SeqCst);
        Ok(SubprocessOutcome {
            exit_code: status.code().map(i64::from),
            signal: signal_name(status),
        })
    })
}

pub struct Setup {
    pub ctx: Context,
    pub shell: Service<Shell>,
    pub executor_fiber: hoh_kernel::Fiber,
    pub manager_disposer: Disposer,
}

pub async fn setup(config: serde_json::Value) -> Setup {
    let ctx = hoh_kernel::Kernel::start().root();
    let runtime = Arc::new(LocalRuntime::new());
    let manager_disposer = runtime.provide(&ctx).await.unwrap();
    let mut config = config;
    // `{ graceMs: 200, ...config }`: a short kill grace unless the case sets its own.
    if config.get("graceMs").is_none() {
        config["graceMs"] = serde_json::json!(200);
    }
    let executor_fiber = ctx
        .plugin::<hoh_bash_local::LocalBashExecutor>(config)
        .unwrap();
    executor_fiber.settled().await.unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    Setup {
        ctx,
        shell,
        executor_fiber,
        manager_disposer,
    }
}

pub async fn setup_fake(
    runtime: Arc<dyn SubprocessRuntime>,
    config: serde_json::Value,
) -> (Context, Service<Shell>) {
    let ctx = hoh_kernel::Kernel::start().root();
    provide_subprocess(&ctx, runtime).await.unwrap();
    let fiber = ctx
        .plugin::<hoh_bash_local::LocalBashExecutor>(config)
        .unwrap();
    fiber.settled().await.unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    (ctx, shell)
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

/// `resolve` throws in the source; through the trait it panics with the same message.
pub fn resolve_error(shell: &Service<Shell>, request: ShellExecRequest) -> String {
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shell.resolve(&request)));
    match outcome {
        Ok(spec) => panic!("resolve unexpectedly succeeded: {spec:?}"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .unwrap_or_default(),
    }
}

/// Historical foreground shorthand over the unified execute() seam.
pub async fn run(shell: &Service<Shell>, spec: ShellExecSpec) -> Result<ShellRunResult, BoxError> {
    let execution = shell.execute(spec).await?;
    execution.result().await
}

/// Historical background shorthand: execute with no deadline armed.
pub async fn start(shell: &Service<Shell>, mut spec: ShellExecSpec) -> Arc<dyn ShellExecution> {
    spec.on_expiry = ShellExpiryPolicy::None;
    shell.execute(spec).await.unwrap()
}

/// Poll a handle's consuming read until the ACCUMULATED delta contains `expected`.
pub async fn read_until(process: &dyn ShellProcess, expected: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut all = String::new();
    while std::time::Instant::now() < deadline {
        all.push_str(&process.read_output().delta);
        if all.contains(expected) {
            return all;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process output did not include {expected:?}; accumulated {all:?}");
}
