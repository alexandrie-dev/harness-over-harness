//! Spawn, stream, direct-result and managed-range lifecycles.
#[cfg(test)]
mod tests;
use crate::{
    error,
    linux_scope::{
        DirectRange, LinuxScopeInternals, LinuxScopeStartup, SystemdScopeOwner, scope_args,
        unit_stem,
    },
    lock,
    managed_owner::{BoundProcessOwner, RangeExit, wait_with_abort},
    output::{
        ManagedProcessBinding, OutputCollector, SpillFailureReporter, SpillOptions, SpillStorage,
        prepare_managed_process_binding,
    },
    runner_launch::*,
    runner_protocol::*,
};
use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Context;
use hoh_subprocess::*;
use hoh_timeout::{AbortError, AbortSignal};
use std::{
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};

pub use crate::runner_launch::child_env;
#[derive(Clone, Default)]
pub struct SpawnInternals {
    pub spill_dir: Option<std::path::PathBuf>,
    pub on_spill_failure: Option<SpillFailureReporter>,
    pub linux_scope: Option<(LinuxScopeInternals, Vec<String>)>,
    pub runner_invocation: Option<Vec<String>>,
    pub spill_storage: Option<Arc<SpillStorage>>,
}
pub fn validate_subprocess_spec(spec: &SubprocessSpawnSpec) -> Result<(), BoxError> {
    if !spec.grace_ms.is_finite()
        || spec.grace_ms <= 0.0
        || spec.grace_ms > hoh_timeout::MAX_TIMER_DELAY_MS as f64
    {
        return Err(error(format!(
            "subprocess graceMs must be a positive finite number no greater than {}",
            hoh_timeout::MAX_TIMER_DELAY_MS
        )));
    }
    if let Some(signal) = &spec.signal
        && signal.aborted()
    {
        let reason = signal
            .reason()
            .map(|e| {
                if e.downcast_ref::<crate::SubprocessAbortReason>()
                    .is_some_and(crate::SubprocessAbortReason::nullish)
                {
                    return "aborted".into();
                }
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::error_text(e.get_ref())
                }))
                .unwrap_or_else(|_| "aborted".into())
            })
            .unwrap_or_else(|| "aborted".into());
        return Err(Box::new(AbortError::new(format!(
            "aborted before spawn: {reason}"
        ))));
    }
    if spec.argv.first().is_none_or(String::is_empty) {
        return Err(SubprocessError::new(
            "invalid argv: expected a non-empty program name at argv[0]",
            SubprocessErrorCode::InvalidArgv,
        )
        .into());
    }
    Ok(())
}
pub fn signal_name(number: i32) -> Option<&'static str> {
    match number {
        1 => Some("SIGHUP"),
        2 => Some("SIGINT"),
        3 => Some("SIGQUIT"),
        4 => Some("SIGILL"),
        5 => Some("SIGTRAP"),
        6 => Some("SIGABRT"),
        7 => Some("SIGBUS"),
        8 => Some("SIGFPE"),
        9 => Some("SIGKILL"),
        10 => Some("SIGUSR1"),
        11 => Some("SIGSEGV"),
        12 => Some("SIGUSR2"),
        13 => Some("SIGPIPE"),
        14 => Some("SIGALRM"),
        15 => Some("SIGTERM"),
        16 => Some("SIGSTKFLT"),
        17 => Some("SIGCHLD"),
        18 => Some("SIGCONT"),
        19 => Some("SIGSTOP"),
        20 => Some("SIGTSTP"),
        21 => Some("SIGTTIN"),
        22 => Some("SIGTTOU"),
        23 => Some("SIGURG"),
        24 => Some("SIGXCPU"),
        25 => Some("SIGXFSZ"),
        26 => Some("SIGVTALRM"),
        27 => Some("SIGPROF"),
        28 => Some("SIGWINCH"),
        29 => Some("SIGIO"),
        30 => Some("SIGPWR"),
        31 => Some("SIGSYS"),
        _ => None,
    }
}
pub fn outcome(status: std::process::ExitStatus) -> SubprocessOutcome {
    use std::os::unix::process::ExitStatusExt;
    SubprocessOutcome {
        exit_code: status.code().map(i64::from),
        signal: status.signal().and_then(signal_name).map(Into::into),
    }
}
pub fn kill_group(pid: Option<u32>, signal: rustix::process::Signal) {
    if let Some(pid) = pid.and_then(|p| rustix::process::Pid::from_raw(p as i32)) {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}
pub fn signal_direct_process(pid: u32, signal: rustix::process::Signal) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(pid as i32) else {
        return false;
    };
    rustix::process::kill_process(pid, signal).is_ok()
        || rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
}
pub(crate) fn signal_child_group(pid: u32, signal: rustix::process::Signal) -> bool {
    let Some(target) = rustix::process::Pid::from_raw(pid as i32) else {
        return false;
    };
    signal_child_group_with(signal, |operation| match operation {
        ChildSignal::Group => rustix::process::kill_process_group(target, signal),
        ChildSignal::Direct => rustix::process::kill_process(target, signal),
        ChildSignal::Probe => rustix::process::test_kill_process(target),
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildSignal {
    Group,
    Direct,
    Probe,
}
fn signal_child_group_with(
    signal: rustix::process::Signal,
    mut operate: impl FnMut(ChildSignal) -> Result<(), rustix::io::Errno>,
) -> bool {
    if operate(ChildSignal::Group).is_ok() && signal == rustix::process::Signal::TERM {
        return true;
    }
    operate(ChildSignal::Direct).is_ok()
        || operate(ChildSignal::Probe) == Err(rustix::io::Errno::SRCH)
}

#[cfg(test)]
mod signal_tests {
    use super::*;
    #[test]
    fn final_kill_targets_group_and_direct_and_probes_after_failed_direct_submission() {
        for group in [false, true] {
            for delivery in [false, true] {
                for probe in [
                    Ok(()),
                    Err(rustix::io::Errno::PERM),
                    Err(rustix::io::Errno::SRCH),
                ] {
                    let mut calls = vec![];
                    let accepted =
                        signal_child_group_with(rustix::process::Signal::KILL, |operation| {
                            calls.push(operation);
                            match operation {
                                ChildSignal::Group if group => Ok(()),
                                ChildSignal::Direct if delivery => Ok(()),
                                ChildSignal::Probe => probe,
                                _ => Err(rustix::io::Errno::PERM),
                            }
                        });
                    assert_eq!(accepted, delivery || probe == Err(rustix::io::Errno::SRCH));
                    assert_eq!(
                        calls,
                        if delivery {
                            vec![ChildSignal::Group, ChildSignal::Direct]
                        } else {
                            vec![ChildSignal::Group, ChildSignal::Direct, ChildSignal::Probe]
                        }
                    );
                }
            }
        }
    }
    #[test]
    fn successful_group_term_needs_no_direct_signal_or_probe() {
        let mut calls = vec![];
        assert!(signal_child_group_with(
            rustix::process::Signal::TERM,
            |operation| {
                calls.push(operation);
                Ok(())
            }
        ));
        assert_eq!(calls, [ChildSignal::Group]);
    }
}

#[derive(Clone)]
pub struct FallbackOwner(Arc<FallbackInner>);
trait FallbackOperations: Send + Sync {
    fn probe_group(&self, pid: rustix::process::Pid) -> Result<(), rustix::io::Errno>;
    fn signal_group(
        &self,
        pid: rustix::process::Pid,
        signal: rustix::process::Signal,
    ) -> Result<(), rustix::io::Errno>;
    fn signal_direct(&self, pid: u32, signal: rustix::process::Signal);
    fn live_member(&self, pid: u32, known: Option<u32>) -> Option<Option<u32>>;
}
struct NativeFallbackOperations;
impl FallbackOperations for NativeFallbackOperations {
    fn probe_group(&self, pid: rustix::process::Pid) -> Result<(), rustix::io::Errno> {
        rustix::process::test_kill_process_group(pid)
    }
    fn signal_group(
        &self,
        pid: rustix::process::Pid,
        signal: rustix::process::Signal,
    ) -> Result<(), rustix::io::Errno> {
        rustix::process::kill_process_group(pid, signal)
    }
    fn signal_direct(&self, pid: u32, signal: rustix::process::Signal) {
        signal_direct_process(pid, signal);
    }
    fn live_member(&self, pid: u32, known: Option<u32>) -> Option<Option<u32>> {
        crate::process_inspector::linux_process_group_live_member(
            pid,
            known,
            Arc::new(crate::process_inspector::NativeProcessInspector),
        )
    }
}
struct FallbackInner {
    pid: Option<u32>,
    direct_settled: Arc<AtomicBool>,
    state: Mutex<(bool, Option<u32>, Option<RangeExit>)>,
    operations: Arc<dyn FallbackOperations>,
}
impl FallbackOwner {
    pub fn new(pid: Option<u32>, direct_settled: Arc<AtomicBool>) -> Self {
        Self::with_operations(pid, direct_settled, Arc::new(NativeFallbackOperations))
    }
    fn with_operations(
        pid: Option<u32>,
        direct_settled: Arc<AtomicBool>,
        operations: Arc<dyn FallbackOperations>,
    ) -> Self {
        Self(Arc::new(FallbackInner {
            pid,
            direct_settled,
            state: Mutex::new((false, None, None)),
            operations,
        }))
    }
    fn signal_tree(&self, signal: rustix::process::Signal) {
        if let Some(pid) = self.0.pid
            && let Some(target) = rustix::process::Pid::from_raw(pid as i32)
            && self.0.operations.signal_group(target, signal).is_err()
            && !self.0.direct_settled.load(Ordering::Acquire)
        {
            self.0.operations.signal_direct(pid, signal);
        }
    }
    fn alive(&self) -> bool {
        let (stopped, known) = {
            let state = lock(&self.0.state);
            (state.0, state.1)
        };
        if stopped {
            return false;
        }
        let Some(pid) = self
            .0
            .pid
            .and_then(|id| rustix::process::Pid::from_raw(id as i32))
        else {
            return false;
        };
        match self.0.operations.probe_group(pid) {
            Ok(()) => {
                if self.0.direct_settled.load(Ordering::Acquire) {
                    match self
                        .0
                        .operations
                        .live_member(pid.as_raw_nonzero().get() as u32, known)
                    {
                        Some(None) => return false,
                        Some(Some(member)) => lock(&self.0.state).1 = Some(member),
                        None => {}
                    }
                }
                true
            }
            Err(rustix::io::Errno::SRCH) => !self.0.direct_settled.load(Ordering::Acquire),
            Err(rustix::io::Errno::PERM) => true,
            Err(_) => !self.0.direct_settled.load(Ordering::Acquire),
        }
    }
}
impl BoundProcessOwner for FallbackOwner {
    fn signal(&self, signal: rustix::process::Signal) {
        if !self.alive() {
            lock(&self.0.state).0 = true;
            return;
        }
        self.signal_tree(signal);
    }
    fn wait_for_exit(&self) -> RangeExit {
        let mut state = lock(&self.0.state);
        if state.0 {
            return futures_util::future::ready(Ok(())).boxed().shared();
        }
        if let Some(future) = &state.2 {
            return future.clone();
        }
        let owner = self.clone();
        let future = async move {
            loop {
                let check = owner.clone();
                if !tokio::task::spawn_blocking(move || check.alive())
                    .await
                    .map_err(SharedError::new)?
                {
                    lock(&owner.0.state).0 = true;
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }
        .boxed()
        .shared();
        state.2 = Some(future.clone());
        future
    }
    fn terminate_for_host_exit(&self) {
        if !lock(&self.0.state).0 {
            self.signal_tree(rustix::process::Signal::KILL);
        }
    }
}
struct BootstrapOwner {
    owner: Arc<dyn BoundProcessOwner>,
    files: LinuxLaunchFiles,
    startup: Option<LinuxScopeStartup>,
}
impl BoundProcessOwner for BootstrapOwner {
    fn signal(&self, s: rustix::process::Signal) {
        if let Some(startup) = &self.startup
            && let Some(signal) = signal_name(s.as_raw())
        {
            lock(&startup.termination_signals).insert(signal.into());
        }
        self.owner.signal(s);
    }
    fn wait_for_exit(&self) -> RangeExit {
        self.owner.wait_for_exit()
    }
    fn terminate_for_host_exit(&self) {
        if let Some(startup) = &self.startup {
            lock(&startup.termination_signals).insert("SIGKILL".into());
        }
        self.owner.terminate_for_host_exit();
    }
    fn cleanup(&self) -> Result<(), BoxError> {
        cleanup_linux_launch_files(&self.files);
        Ok(())
    }
}

pub struct ManagedProcessLaunch {
    pub stdin: Option<SubprocessWritable>,
    pub stdout: Option<SubprocessReadable>,
    pub stderr: Option<SubprocessReadable>,
    pub control: Option<SubprocessControl>,
    pub direct: SubprocessDone,
    pub owner: Arc<dyn BoundProcessOwner>,
}
struct RegisteredChild {
    child: tokio::process::Child,
    owner: Arc<dyn BoundProcessOwner>,
    armed: bool,
}
impl RegisteredChild {
    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let result = self.child.wait().await;
        self.armed = false;
        result
    }
}
impl Drop for RegisteredChild {
    fn drop(&mut self) {
        if self.armed {
            crate::shim::terminate_for_host_exit(|| self.owner.terminate_for_host_exit());
        }
    }
}
pub struct LocalSubprocessHandle {
    pub handle: SubprocessHandle,
    pub lifecycle: Arc<LocalProcessLifecycle>,
    pub(crate) control_gate: Option<Arc<ControlGate>>,
}
pub(crate) struct ControlGate {
    closed: AtomicBool,
    read: futures_util::task::AtomicWaker,
    write: futures_util::task::AtomicWaker,
}
impl ControlGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            read: futures_util::task::AtomicWaker::new(),
            write: futures_util::task::AtomicWaker::new(),
        })
    }
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.read.wake();
        self.write.wake();
    }
}
struct ControlledChannel {
    inner: SubprocessControl,
    gate: Arc<ControlGate>,
}
impl AsyncRead for ControlledChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.gate.read.register(cx.waker());
        if self.gate.closed.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl tokio::io::AsyncWrite for ControlledChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.gate.write.register(cx.waker());
        if self.gate.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        self.gate.write.register(cx.waker());
        if self.gate.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.gate.write.register(cx.waker());
        if self.gate.closed.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
impl std::ops::Deref for LocalSubprocessHandle {
    type Target = SubprocessHandle;
    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}
impl LocalSubprocessHandle {
    pub fn terminate_for_host_exit(&self) {
        self.lifecycle.owner.terminate_for_host_exit();
    }
}
pub struct LocalProcessLifecycle {
    pub owner: Arc<dyn BoundProcessOwner>,
    pub done: SubprocessDone,
    termination_requested: AtomicBool,
    range_observed: Arc<AtomicBool>,
    term: tokio::sync::Notify,
    range_done: tokio::sync::Notify,
    observation: Arc<Mutex<Option<RangeExit>>>,
}
impl LocalProcessLifecycle {
    pub fn range_observed(&self) -> bool {
        self.range_observed.load(Ordering::Acquire)
    }
    pub fn terminate_for_host_exit(&self) {
        if !self.range_observed() {
            self.owner.terminate_for_host_exit();
        }
    }
    fn range_exit(&self) -> RangeExit {
        let mut state = lock(&self.observation);
        if let Some(pending) = &*state {
            return pending.clone();
        }
        let owner = self.owner.clone();
        let done = self.done.clone();
        let state_ref = Arc::downgrade(&self.observation);
        let pending = async move {
            let result = owner.wait_for_exit().await;
            if result.is_err()
                && done.peek().is_none()
                && let Some(state) = state_ref.upgrade()
            {
                *lock(&state) = None;
            }
            result
        }
        .boxed()
        .shared();
        *state = Some(pending.clone());
        pending
    }
}
impl SubprocessLifecycle for LocalProcessLifecycle {
    fn terminate(&self) {
        if !self.range_observed() && !self.termination_requested.swap(true, Ordering::AcqRel) {
            self.term.notify_one();
        }
    }
    fn wait_for_exit(&self, signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        let pending = self.range_exit();
        let observed = self.range_observed.clone();
        Box::pin(async move {
            if observed.load(Ordering::Acquire) {
                return Ok(true);
            }
            let complete = wait_with_abort(pending, signal).await?;
            if complete {
                observed.store(true, Ordering::Release);
                self.range_done.notify_waiters();
            }
            Ok(complete)
        })
    }
}
struct ObservedRead {
    inner: SubprocessReadable,
    closed: Option<tokio::sync::oneshot::Sender<()>>,
}
impl AsyncRead for ObservedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        if (matches!(result, Poll::Ready(Err(_)))
            || matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() == before)
            && let Some(sender) = self.closed.take()
        {
            let _ = sender.send(());
        }
        result
    }
}
impl Drop for ObservedRead {
    fn drop(&mut self) {
        if let Some(sender) = self.closed.take() {
            let _ = sender.send(());
        }
    }
}
pub(crate) fn grace(ms: f64) -> Duration {
    let ms = if ms.is_nan() {
        1.0
    } else {
        ms.clamp(1.0, hoh_timeout::MAX_TIMER_DELAY_MS as f64)
    };
    Duration::from_millis(ms as u64)
}
type StreamClosed = futures_util::future::Shared<BoxFuture<'static, ()>>;
struct StreamBinding {
    stream: Option<SubprocessReadable>,
    collector: Option<Arc<OutputCollector>>,
    closed: StreamClosed,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
fn bind_stream(
    ctx: &Context,
    stream: Option<SubprocessReadable>,
    mode: &SubprocessOutputMode,
    label: &str,
    binding: &ManagedProcessBinding,
) -> Result<StreamBinding, BoxError> {
    let empty = || StreamBinding {
        stream: None,
        collector: None,
        closed: async {}.boxed().shared(),
        stop: None,
    };
    let Some(stream) = stream else {
        return Ok(empty());
    };
    match mode {
        SubprocessOutputMode::Inherit => Ok(empty()),
        SubprocessOutputMode::Pipe => {
            let (send, receive) = tokio::sync::oneshot::channel();
            Ok(StreamBinding {
                stream: Some(Box::new(ObservedRead {
                    inner: stream,
                    closed: Some(send),
                })),
                collector: None,
                closed: async move {
                    let _ = receive.await;
                }
                .boxed()
                .shared(),
                stop: None,
            })
        }
        SubprocessOutputMode::Collect(mode) => {
            let collector = Arc::new(OutputCollector::new(
                mode.max_bytes,
                label,
                mode.spill.as_ref().map(|spill| SpillOptions {
                    max_bytes: spill.max_bytes,
                    dir: binding.spill_dir.clone(),
                    on_failure: binding.on_spill_failure.clone(),
                }),
            ));
            let result = collector.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            let (stop, mut stopped) = tokio::sync::oneshot::channel();
            let label = label.to_owned();
            ctx.spawn(format!("subprocess {label} collection"),move|_cancel|async move {
                let mut stream=stream;let mut buffer=vec![0;65536];
                loop{let read=tokio::select!{read=stream.read(&mut buffer)=>read,_=&mut stopped=>{break;}};match read{Ok(0)|Err(_)=>break,Ok(count)=>{let collector=result.clone();let bytes=buffer[..count].to_vec();let _=tokio::task::spawn_blocking(move||collector.push(&bytes)).await;}}}
                let sealed=result.clone();let _=tokio::task::spawn_blocking(move||sealed.seal()).await;let _=send.send(());Ok(())
            })?;
            Ok(StreamBinding {
                stream: None,
                collector: Some(collector),
                closed: async move {
                    let _ = receive.await;
                }
                .boxed()
                .shared(),
                stop: Some(stop),
            })
        }
    }
}
pub fn bind_managed_process(
    ctx: &Context,
    spec: SubprocessSpawnSpec,
    mut launch: ManagedProcessLaunch,
    binding: ManagedProcessBinding,
) -> Result<LocalSubprocessHandle, BoxError> {
    let control_gate = launch.control.as_ref().map(|_| ControlGate::new());
    let control = launch
        .control
        .take()
        .zip(control_gate.clone())
        .map(|(inner, gate)| Box::new(ControlledChannel { inner, gate }) as SubprocessControl);
    let out_binding = bind_stream(
        ctx,
        launch.stdout.take(),
        &spec.stdio.stdout,
        "stdout",
        &binding,
    )?;
    let err_binding = bind_stream(
        ctx,
        launch.stderr.take(),
        &spec.stdio.stderr,
        "stderr",
        &binding,
    )?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let done = subprocess_done(async move { receive.await.map_err(SharedError::new)? });
    let lifecycle = Arc::new(LocalProcessLifecycle {
        owner: launch.owner.clone(),
        done: done.clone(),
        termination_requested: AtomicBool::new(false),
        range_observed: Arc::new(AtomicBool::new(false)),
        term: tokio::sync::Notify::new(),
        range_done: tokio::sync::Notify::new(),
        observation: Arc::default(),
    });
    let local = lifecycle.clone();
    let owner = launch.owner.clone();
    let spec_signal = spec.signal.clone();
    let delay = grace(spec.grace_ms);
    ctx.spawn("subprocess termination",move|cancel|async move{
        if local.range_observed(){return Ok(());}
        tokio::select! {biased;_ = local.range_done.notified()=>return Ok(()),_ = local.term.notified()=>{},_ = cancel.cancelled()=>{},_ = async {if let Some(signal)=spec_signal{signal.cancelled().await;}else{std::future::pending::<()>().await;}}=>{}}
        if local.range_observed(){return Ok(());}local.termination_requested.store(true,Ordering::Release);
        let signal=owner.clone();tokio::task::spawn_blocking(move||signal.signal(rustix::process::Signal::TERM)).await?;
        let pending=local.range_exit();tokio::pin!(pending);let deadline=tokio::time::Instant::now()+delay;
        tokio::select!{result=&mut pending=>{if result.is_ok(){local.range_observed.store(true,Ordering::Release);local.range_done.notify_waiters();return Ok(());}tokio::time::sleep_until(deadline).await;},_=tokio::time::sleep_until(deadline)=>{}}
        if !local.range_observed(){tokio::task::spawn_blocking(move||owner.signal(rustix::process::Signal::KILL)).await?;}Ok(())
    })?;
    let StreamBinding {
        stream: stdout,
        collector: out,
        closed: out_closed,
        stop: out_stop,
    } = out_binding;
    let StreamBinding {
        stream: stderr,
        collector: err,
        closed: err_closed,
        stop: err_stop,
    } = err_binding;
    let local = lifecycle.clone();
    let storage = binding.storage.clone();
    ctx.spawn("subprocess result", move |_cancel| async move {
        let _storage = storage;
        let result = launch.direct.await;
        if result.is_ok() {
            let closed = futures_util::future::join(out_closed.clone(), err_closed.clone());
            tokio::select! {_=closed=>{},_=tokio::time::sleep(delay)=>{}}
        } else {
            local.terminate();
        }
        if let Some(stop) = out_stop {
            let _ = stop.send(());
            out_closed.await;
        }
        if let Some(stop) = err_stop {
            let _ = stop.send(());
            err_closed.await;
        }
        let _ = send.send(result);
        Ok(())
    })?;
    let local = lifecycle.clone();
    ctx.spawn("subprocess range", move |_cancel| async move {
        let _ = local.done.clone().await;
        let result = local.range_exit().await;
        if result.is_ok() {
            local.range_observed.store(true, Ordering::Release);
            local.range_done.notify_waiters();
        }
        let owner = local.owner.clone();
        let _ = tokio::task::spawn_blocking(move || owner.cleanup()).await;
        Ok(())
    })?;
    let stdin = match spec.stdio.stdin {
        SubprocessStdinMode::Pipe => launch.stdin,
        SubprocessStdinMode::Ignore => None,
        SubprocessStdinMode::Data { data } => {
            if let Some(mut input) = launch.stdin.take() {
                ctx.spawn("subprocess batch stdin",move|cancel|async move{tokio::select!{_ = async{let _=input.write_all(data.as_bytes()).await;let _=input.shutdown().await;}=>{},_=cancel.cancelled()=>{}}Ok(())})?;
            }
            None
        }
    };
    Ok(LocalSubprocessHandle {
        handle: SubprocessHandle {
            stdin,
            stdout,
            stderr,
            control,
            collected: SubprocessCollectedOutputs {
                stdout: out.map(|s| s as Arc<dyn SubprocessOutputReader>),
                stderr: err.map(|s| s as Arc<dyn SubprocessOutputReader>),
            },
            done,
            lifecycle: lifecycle.clone(),
        },
        lifecycle,
        control_gate,
    })
}

/// Source fallback with a private Rust exec helper to establish the session before target exec.
pub fn spawn_subprocess(
    ctx: &Context,
    spec: SubprocessSpawnSpec,
    internals: SpawnInternals,
) -> Result<LocalSubprocessHandle, BoxError> {
    validate_subprocess_spec(&spec)?;
    let environment = target_environment(&spec)?;
    let storage = internals
        .spill_storage
        .clone()
        .unwrap_or_else(|| Arc::new(SpillStorage::default()));
    let report = internals
        .on_spill_failure
        .clone()
        .unwrap_or_else(crate::output::stderr_spill_failure);
    let binding = prepare_managed_process_binding(&storage, internals.spill_dir.clone(), report)?;
    let invocation = internals
        .runner_invocation
        .clone()
        .or_else(|| {
            internals
                .linux_scope
                .as_ref()
                .map(|(scope, _)| scope.runner_invocation.clone())
        })
        .unwrap_or_else(spawn_runner_invocation);
    let pipes = [
        spec.stdio.stdin == SubprocessStdinMode::Pipe
            || matches!(spec.stdio.stdin, SubprocessStdinMode::Data { .. }),
        !matches!(spec.stdio.stdout, SubprocessOutputMode::Inherit),
        !matches!(spec.stdio.stderr, SubprocessOutputMode::Inherit),
    ];
    let request = LinuxLaunchRequest {
        cwd: spec.cwd.clone(),
        env: environment.clone(),
        pipe_fds: Some(
            pipes
                .iter()
                .enumerate()
                .filter_map(|(n, on)| on.then_some(n as u64))
                .collect(),
        ),
        control: spec.stdio.control,
        control_path: None,
    };
    let files = create_linux_launch_files(&request)?;
    let startup = LinuxScopeStartup::new(
        files.clone(),
        crate::linux_scope::LinuxScopeKind::Subprocess,
    );
    let mut command;
    let native = internals.linux_scope.clone();
    let bootstrap = runner_invocation_available(&invocation);
    if let Some((scope, literal)) = &native {
        // The single-threaded helper performs Node detached:true's setsid before
        // exec, retaining the direct PID as both session and process-group leader.
        let detached = spawn_runner_invocation();
        command = tokio::process::Command::new(&detached[0]);
        let unit = unit_stem("dsh-subprocess")?;
        command
            .args(&detached[1..])
            .arg(DETACHED_EXEC_MODE)
            .arg("--")
            .arg(&scope.systemd_run)
            .args(scope_args(&unit, literal, &invocation, &spec.argv));
        command.env_clear().envs(runner_environment(
            &files.request_path.to_string_lossy(),
            Some(&invocation),
        ));
        return launch_and_bind(
            ctx,
            spec,
            command,
            LaunchInputs {
                files,
                startup,
                binding,
                native: Some((scope.clone(), format!("{unit}.scope"))),
                bootstrap: true,
            },
        );
    }
    if bootstrap {
        command = tokio::process::Command::new(&invocation[0]);
        command.args(&invocation[1..]).arg("--").args(&spec.argv);
        command.env_clear().envs(runner_environment(
            &files.request_path.to_string_lossy(),
            Some(&invocation),
        ));
    } else {
        if spec.stdio.control.is_some() {
            cleanup_linux_launch_files(&files);
            return Err(error(
                "subprocess-local: inherited control requires the hoh-subprocess-runner executable",
            ));
        }
        command = tokio::process::Command::new(&spec.argv[0]);
        command
            .args(&spec.argv[1..])
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(environment);
        command.process_group(0);
    }
    launch_and_bind(
        ctx,
        spec,
        command,
        LaunchInputs {
            files,
            startup,
            binding,
            native: None,
            bootstrap,
        },
    )
}
struct LaunchInputs {
    files: LinuxLaunchFiles,
    startup: LinuxScopeStartup,
    binding: ManagedProcessBinding,
    native: Option<(LinuxScopeInternals, String)>,
    bootstrap: bool,
}
struct UnboundLaunch {
    files: Option<LinuxLaunchFiles>,
    owner: Option<Arc<dyn BoundProcessOwner>>,
}
impl UnboundLaunch {
    fn commit(&mut self) {
        self.files = None;
        self.owner = None;
    }
}
impl Drop for UnboundLaunch {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            crate::shim::terminate_for_host_exit(|| owner.terminate_for_host_exit());
        }
        if let Some(files) = &self.files {
            cleanup_linux_launch_files(files);
        }
    }
}
fn launch_and_bind(
    ctx: &Context,
    spec: SubprocessSpawnSpec,
    mut command: tokio::process::Command,
    inputs: LaunchInputs,
) -> Result<LocalSubprocessHandle, BoxError> {
    let LaunchInputs {
        files,
        startup,
        binding,
        native,
        bootstrap,
    } = inputs;
    let mut cleanup = UnboundLaunch {
        files: Some(files.clone()),
        owner: None,
    };
    // Node's POSIX pipe endpoints are AF_UNIX sockets, including fd 0.
    fn pair() -> Result<(Stdio, tokio::net::UnixStream), BoxError> {
        let (parent, child) = std::os::unix::net::UnixStream::pair()?;
        parent.set_nonblocking(true)?;
        Ok((
            Stdio::from(std::os::fd::OwnedFd::from(child)),
            tokio::net::UnixStream::from_std(parent)?,
        ))
    }
    let (input, stdin) = if matches!(spec.stdio.stdin, SubprocessStdinMode::Ignore) {
        (Stdio::null(), None)
    } else {
        let (child, parent) = pair()?;
        (child, Some(Box::new(parent) as SubprocessWritable))
    };
    let (output, stdout) = if matches!(spec.stdio.stdout, SubprocessOutputMode::Inherit) {
        (Stdio::inherit(), None)
    } else {
        let (child, parent) = pair()?;
        (child, Some(Box::new(parent) as SubprocessReadable))
    };
    let (errors, stderr) = if matches!(spec.stdio.stderr, SubprocessOutputMode::Inherit) {
        (Stdio::inherit(), None)
    } else {
        let (child, parent) = pair()?;
        (child, Some(Box::new(parent) as SubprocessReadable))
    };
    command
        .stdin(input)
        .stdout(output)
        .stderr(errors)
        .kill_on_drop(true);
    let exec_acknowledgement = if native.is_some() {
        Some(ExecAcknowledgement::create_detached(&files)?)
    } else if bootstrap {
        Some(ExecAcknowledgement::create(&files)?)
    } else {
        None
    };
    // SHIM(node:child_process): safe arbitrary child-fd inheritance is absent from
    // the approved spawn API; the private socket bridge supplies the fd-7 stream.
    let control = if spec.stdio.control.is_some() {
        Some(tokio::net::UnixListener::bind(
            files
                .control_path
                .as_ref()
                .ok_or_else(|| error("subprocess control path is absent"))?,
        )?)
    } else {
        None
    };
    let actual = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let spawned = command.spawn();
    let (direct_send, direct_receive) =
        tokio::sync::oneshot::channel::<Result<SubprocessOutcome, SharedError>>();
    let direct = subprocess_done(async move { direct_receive.await.map_err(SharedError::new)? });
    let direct_settled = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let child = match spawned {
        Ok(child) => child,
        Err(error) => {
            running.store(false, Ordering::Release);
            direct_settled.store(true, Ordering::Release);
            let _ = direct_send.send(Err(SharedError::new(SubprocessSpawnRejection::new(
                error, &actual,
            ))));
            let owner = Arc::new(BootstrapOwner {
                owner: Arc::new(FallbackOwner::new(None, direct_settled)),
                files,
                startup: None,
            });
            let result = bind_managed_process(
                ctx,
                spec,
                ManagedProcessLaunch {
                    stdin,
                    stdout,
                    stderr,
                    control: None,
                    direct,
                    owner,
                },
                binding,
            );
            if result.is_ok() {
                cleanup.commit();
            }
            return result;
        }
    };
    let pid = child.id();
    let (exit_send, exit_receive) = tokio::sync::oneshot::channel::<()>();
    let exit = async move {
        let _ = exit_receive.await;
        Ok(())
    }
    .boxed()
    .shared();
    let reject_cancelled_fallback_startup = native.is_none() && bootstrap;
    let owner: Arc<dyn BoundProcessOwner> = match native {
        Some((internals, unit)) => {
            let direct_pid = pid;
            Arc::new(SystemdScopeOwner::new(
                unit,
                startup.clone(),
                DirectRange {
                    running: running.clone(),
                    signal: Arc::new(move |signal| {
                        direct_pid.is_some_and(|pid| signal_child_group(pid, signal))
                    }),
                    settled: exit,
                },
                internals,
            ))
        }
        None => Arc::new(BootstrapOwner {
            owner: Arc::new(FallbackOwner::new(pid, direct_settled.clone())),
            files: files.clone(),
            startup: bootstrap.then(|| startup.clone()),
        }),
    };
    let stopped = direct_settled.clone();
    cleanup.owner = Some(owner.clone());
    let active = running.clone();
    let acknowledgement_abort = spec.signal.clone();
    let mut child = RegisteredChild {
        child,
        owner: owner.clone(),
        armed: true,
    };
    ctx.spawn("subprocess direct exit", move |cancel| async move {
        enum Acknowledgement {
            Ready(Result<(), BoxError>),
            ContextCancelled,
            Aborted,
        }
        let acknowledgement = if let Some(mut acknowledgement) = exec_acknowledgement {
            tokio::select! {
                biased;
                result = acknowledgement.wait(&mut child.child) => Acknowledgement::Ready(result),
                _ = cancel.cancelled() => Acknowledgement::ContextCancelled,
                _ = async move {
                    if let Some(signal) = acknowledgement_abort {
                        let _ = signal.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => Acknowledgement::Aborted,
            }
        } else {
            Acknowledgement::Ready(Ok(()))
        };
        let context_cancelled_during_startup =
            matches!(&acknowledgement, Acknowledgement::ContextCancelled);
        let result =
            match acknowledgement {
                Acknowledgement::Ready(Ok(()))
                | Acknowledgement::ContextCancelled
                | Acknowledgement::Aborted => child.wait().await.map(outcome).map_err(|error| {
                    SharedError::new(SubprocessSpawnRejection::new(error, &actual))
                }),
                Acknowledgement::Ready(Err(error)) => {
                    child.owner.terminate_for_host_exit();
                    let _ = child.child.start_kill();
                    let _ = child.wait().await;
                    Err(SharedError::new(error))
                }
            };
        stopped.store(true, Ordering::Release);
        active.store(false, Ordering::Release);
        let _ = exit_send.send(());
        let result = if bootstrap {
            match result {
                Ok(outcome) => {
                    let resolved =
                        tokio::task::spawn_blocking(move || startup.resolve_outcome(outcome))
                            .await
                            .map_err(SharedError::new)?;
                    if context_cancelled_during_startup
                        && reject_cancelled_fallback_startup
                        && resolved.is_ok()
                    {
                        Err(SharedError::new(error(
                            "subprocess fallback startup was cancelled before exec acknowledgement",
                        )))
                    } else {
                        resolved.map_err(SharedError::new)
                    }
                }
                Err(error) => Err(error),
            }
        } else {
            result
        };
        let _ = direct_send.send(result);
        Ok(())
    })?;
    let control = if let Some(listener) = control {
        let (client, mut server) = tokio::io::duplex(65536);
        let wait = owner.clone();
        let result = direct.clone();
        ctx.spawn("subprocess control pipe",move|cancel|async move{
            let rejected = result.clone();
            tokio::select!{
                accepted=listener.accept()=>{
                    if let Ok((mut stream,_))=accepted {
                        let rejected=result.clone();
                        tokio::select!{
                            _=tokio::io::copy_bidirectional(&mut stream,&mut server)=>{},
                            _=async move { if rejected.await.is_ok() { std::future::pending::<()>().await } }=>{},
                            _=cancel.cancelled()=>{let _=result.await;let _=wait.wait_for_exit().await;}
                        }
                    }
                },
                _=async move { if rejected.await.is_ok() { std::future::pending::<()>().await } }=>{},
                _=cancel.cancelled()=>{let _=result.await;let _=wait.wait_for_exit().await;}
            }
            Ok(())
        })?;
        Some(Box::new(client) as SubprocessControl)
    } else {
        None
    };
    let result = bind_managed_process(
        ctx,
        spec,
        ManagedProcessLaunch {
            stdin,
            stdout,
            stderr,
            control,
            direct,
            owner,
        },
        binding,
    );
    if result.is_ok() {
        cleanup.commit();
    }
    result
}
