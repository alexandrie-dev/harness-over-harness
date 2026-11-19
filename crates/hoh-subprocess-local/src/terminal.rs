//! Real PTYs with start-identity fences, backpressure and joined teardown.
#[cfg(test)]
pub(crate) mod tests;
use crate::{
    error,
    linux_scope::{
        DirectRange, LinuxScopeInternals, SystemdScopeOwner, prepare_linux_terminal_scope,
    },
    lock,
    managed_owner::{BoundProcessOwner, RangeExit},
    process_inspector::{ProcessIdentity, ProcessInspector, ProcessSnapshot},
    runner_launch::*,
    runner_protocol::*,
    shell_activity::{ShellActivity, prepare_shell_activity},
};
use futures_util::{FutureExt, task::AtomicWaker};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Undo};
use hoh_subprocess::*;
use indexmap::IndexSet;
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, ReadBuf};

struct QueueState {
    chunks: VecDeque<Vec<u8>>,
    offset: usize,
    bytes: usize,
    ended: bool,
    consumer_gone: bool,
}
struct OutputQueue {
    state: Mutex<QueueState>,
    waker: AtomicWaker,
    space: Condvar,
    terminating: AtomicBool,
    #[cfg(test)]
    waiting: AtomicBool,
}
impl OutputQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState {
                chunks: VecDeque::new(),
                offset: 0,
                bytes: 0,
                ended: false,
                consumer_gone: false,
            }),
            waker: AtomicWaker::new(),
            space: Condvar::new(),
            terminating: AtomicBool::new(false),
            #[cfg(test)]
            waiting: AtomicBool::new(false),
        })
    }
    fn push(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let mut state = lock(&self.state);
        while state.bytes >= 65536
            && !state.consumer_gone
            && !self.terminating.load(Ordering::Acquire)
        {
            #[cfg(test)]
            self.waiting.store(true, Ordering::Release);
            state = self
                .space
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(test)]
            self.waiting.store(false, Ordering::Release);
        }
        if state.consumer_gone {
            return false;
        }
        state.bytes += bytes.len();
        state.chunks.push_back(bytes);
        drop(state);
        self.waker.wake();
        true
    }
    fn end(&self) {
        lock(&self.state).ended = true;
        self.waker.wake();
        self.space.notify_all();
    }
    fn resume_for_cleanup(&self) {
        self.terminating.store(true, Ordering::Release);
        self.space.notify_all();
    }
}
struct TerminalOutput(Arc<OutputQueue>);
impl AsyncRead for TerminalOutput {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.0.waker.register(cx.waker());
        let mut state = lock(&self.0.state);
        if let Some(bytes) = state.chunks.front() {
            let count = buf.remaining().min(bytes.len() - state.offset);
            buf.put_slice(&bytes[state.offset..state.offset + count]);
            state.offset += count;
            state.bytes -= count;
            if state
                .chunks
                .front()
                .is_some_and(|b| state.offset == b.len())
            {
                state.chunks.pop_front();
                state.offset = 0;
            }
            self.0.space.notify_all();
            Poll::Ready(Ok(()))
        } else if state.ended || buf.remaining() == 0 {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}
impl Drop for TerminalOutput {
    fn drop(&mut self) {
        lock(&self.0.state).consumer_gone = true;
        self.0.space.notify_all();
    }
}
fn decode_utf8(pending: &mut Vec<u8>, incoming: &[u8], eof: bool) -> Vec<u8> {
    pending.extend_from_slice(incoming);
    let mut output = vec![];
    loop {
        match std::str::from_utf8(pending) {
            Ok(_) => {
                output.append(pending);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                output.extend_from_slice(&pending[..valid]);
                pending.drain(..valid);
                if let Some(length) = error.error_len() {
                    output.extend_from_slice("�".as_bytes());
                    pending.drain(..length);
                } else {
                    if eof && !pending.is_empty() {
                        output.extend_from_slice("�".as_bytes());
                        pending.clear();
                    }
                    break;
                }
            }
        }
    }
    output
}

struct ActivityState {
    key: String,
    revision: u64,
    quiescent: bool,
    range_empty: bool,
}
trait TerminalMaster: Send + Sync {
    fn resize(&self, cols: u16, rows: u16) -> Result<(), BoxError>;
    #[cfg(test)]
    fn dimensions(&self) -> (u16, u16);
}
struct PortableMaster(Mutex<Box<dyn MasterPty + Send>>);
impl TerminalMaster for PortableMaster {
    fn resize(&self, cols: u16, rows: u16) -> Result<(), BoxError> {
        lock(&self.0)
            .resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| error(e.to_string()))
    }
    #[cfg(test)]
    fn dimensions(&self) -> (u16, u16) {
        let size = lock(&self.0).get_size().unwrap();
        (size.cols, size.rows)
    }
}
pub struct LocalTerminalHandle {
    pub pid: u32,
    pub done: SubprocessDone,
    master: Arc<dyn TerminalMaster>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    output: Arc<OutputQueue>,
    inspector: Arc<dyn ProcessInspector>,
    root_identity: Option<ProcessIdentity>,
    descendants: Mutex<Vec<ProcessIdentity>>,
    exited: AtomicBool,
    grace: Duration,
    managed: Option<Arc<dyn BoundProcessOwner>>,
    activity: Option<Arc<ShellActivity>>,
    state: Mutex<ActivityState>,
    cleanup: Mutex<Option<RangeExit>>,
    reader_done: RangeExit,
    writer_done: RangeExit,
    operations: Arc<tokio::sync::RwLock<()>>,
    on_quiescence: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    shell_signal: Arc<dyn Fn(rustix::process::Signal) -> bool + Send + Sync>,
    cleanup_requested: AtomicBool,
    managed_owner_cleaned: AtomicBool,
}
impl LocalTerminalHandle {
    pub fn running(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }
    pub fn on_quiescence(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.on_quiescence) = Some(callback);
        if lock(&self.state).quiescent
            && let Some(callback) = lock(&self.on_quiescence).take()
        {
            callback();
        }
    }
    fn invalidate(&self) {
        if let Some(activity) = &self.activity {
            activity.invalidate();
        }
    }
    fn adopt_descendants(&self, observed: &ProcessSnapshot) -> Vec<ProcessIdentity> {
        let tree = observed.tree(self.pid);
        let root = tree.iter().find(|p| p.pid == self.pid);
        let verified = root
            .zip(self.root_identity.as_ref())
            .is_some_and(|(current, initial)| current.started == initial.started);
        let mut tracked = lock(&self.descendants);
        let mut members = tracked.clone();
        if verified {
            members.extend(tree);
            members.extend(observed.session(self.pid));
        }
        let mut seen = IndexSet::new();
        members.retain(|p| {
            p.pid != self.pid && observed.alive(p) && seen.insert((p.pid, p.started.clone()))
        });
        *tracked = members.clone();
        members
    }
    fn foreground(&self) -> Result<Option<SubprocessTerminalForeground>, BoxError> {
        self.adopt_descendants(&self.inspector.snapshot()?);
        Ok(self
            .inspector
            .foreground_pgid(self.pid)
            .map(|group| SubprocessTerminalForeground {
                process_group_id: u64::from(group),
                input_waiting: self.inspector.is_stdin_waiting(group, self.pid),
            }))
    }
    fn activity_now(&self) -> SubprocessTerminalActivity {
        use SubprocessTerminalActivityState::{Busy, Idle, Unknown};
        let quiescent = lock(&self.state).quiescent;
        let mut state = if quiescent { Idle } else { Unknown };
        let mut revision = 0;
        if !quiescent {
            let inspected = (|| -> Result<SubprocessTerminalActivityState, BoxError> {
                let shell = self
                    .activity
                    .as_ref()
                    .map(|a| a.inspect(self.pid))
                    .unwrap_or(SubprocessTerminalActivity {
                        state: Unknown,
                        revision: 0,
                    });
                revision = shell.revision;
                let observed = self.inspector.snapshot()?;
                let descendants = self.adopt_descendants(&observed);
                let tree = observed.tree(self.pid);
                let root = tree.iter().find(|p| p.pid == self.pid);
                if !self.running() && lock(&self.state).range_empty {
                    return Ok(Idle);
                }
                if !descendants.is_empty() {
                    return Ok(Busy);
                }
                if !self.running() && self.managed.is_none() && observed.complete && root.is_none()
                {
                    return Ok(
                        if observed.session(self.pid).iter().any(|p| observed.alive(p)) {
                            Busy
                        } else {
                            Idle
                        },
                    );
                }
                if observed.complete
                    && root.is_some()
                    && root
                        .zip(self.root_identity.as_ref())
                        .is_some_and(|(a, b)| a.started == b.started)
                {
                    let mut value = match self.inspector.foreground_pgid(self.pid) {
                        None => Unknown,
                        Some(pid) if pid == self.pid => shell.state,
                        Some(_) => Busy,
                    };
                    if value == Idle
                        && let Some(owner) = &self.managed
                    {
                        value = match owner.inspect_task_count()? {
                            Some(1) => Idle,
                            Some(tasks) if tasks > 1 => Busy,
                            _ => Unknown,
                        };
                    }
                    return Ok(value);
                }
                Ok(Unknown)
            })();
            state = inspected.unwrap_or(Unknown);
        }
        let key = format!(
            "{revision}:{}",
            match state {
                Idle => "idle",
                Busy => "busy",
                Unknown => "unknown",
            }
        );
        let mut tracked = lock(&self.state);
        if key != tracked.key {
            tracked.key = key;
            tracked.revision += 1;
        }
        SubprocessTerminalActivity {
            state,
            revision: tracked.revision,
        }
    }
    fn signal_members(&self, members: &[ProcessIdentity], signal: rustix::process::Signal) {
        for member in members {
            let _ = self.inspector.signal_process(member, signal);
        }
    }
    fn union(groups: impl IntoIterator<Item = Vec<ProcessIdentity>>) -> Vec<ProcessIdentity> {
        let mut seen = IndexSet::new();
        groups
            .into_iter()
            .flatten()
            .filter(|p| seen.insert((p.pid, p.started.clone())))
            .collect()
    }
    async fn wait_members(
        self: &Arc<Self>,
        members: Vec<ProcessIdentity>,
    ) -> Result<Vec<ProcessIdentity>, BoxError> {
        if members.is_empty() {
            return Ok(members);
        }
        let until = tokio::time::Instant::now() + self.grace;
        let mut survivors = members;
        loop {
            let target = self.clone();
            let check = survivors.clone();
            survivors = tokio::task::spawn_blocking(move || {
                let observed = target.inspector.snapshot()?;
                Ok::<_, BoxError>(
                    check
                        .into_iter()
                        .filter(|p| observed.alive(p))
                        .collect::<Vec<_>>(),
                )
            })
            .await??;
            if survivors.is_empty() || tokio::time::Instant::now() >= until {
                return Ok(survivors);
            }
            tokio::time::sleep(
                (until - tokio::time::Instant::now()).min(Duration::from_millis(25)),
            )
            .await;
        }
    }
    async fn stop_descendants(self: &Arc<Self>) -> Result<Vec<ProcessIdentity>, BoxError> {
        let handle = self.clone();
        let captured = tokio::task::spawn_blocking(move || {
            let members = handle.adopt_descendants(&handle.inspector.snapshot()?);
            handle.signal_members(&members, rustix::process::Signal::TERM);
            Ok::<_, BoxError>(members)
        })
        .await??;
        let captured = self.wait_members(captured).await?;
        let handle = self.clone();
        let members = tokio::task::spawn_blocking(move || {
            let members = Self::union([
                captured,
                handle.adopt_descendants(&handle.inspector.snapshot()?),
            ]);
            handle.signal_members(&members, rustix::process::Signal::KILL);
            Ok::<_, BoxError>(members)
        })
        .await??;
        let survivors = self.wait_members(members).await?;
        let handle = self.clone();
        tokio::task::spawn_blocking(move || {
            let observed = handle.inspector.snapshot()?;
            Ok(
                Self::union([survivors, handle.adopt_descendants(&observed)])
                    .into_iter()
                    .filter(|p| observed.alive(p))
                    .collect(),
            )
        })
        .await?
    }
    async fn stop_shell(&self) -> Result<(), BoxError> {
        for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
            if !self.running() {
                return Ok(());
            }
            (self.shell_signal)(signal);
            tokio::select! {_=self.done.clone()=>{},_=tokio::time::sleep(self.grace)=>{}}
        }
        if self.running() {
            Err(error(format!(
                "terminal cleanup failed; surviving pid: {}",
                self.pid
            )))
        } else {
            Ok(())
        }
    }
    pub fn terminate_for_host_exit(&self) {
        let members = self
            .inspector
            .snapshot()
            .map(|s| self.adopt_descendants(&s))
            .unwrap_or_else(|_| lock(&self.descendants).clone());
        self.signal_members(&members, rustix::process::Signal::KILL);
        if self.running() {
            if let Some(root) = &self.root_identity {
                let _ = self
                    .inspector
                    .signal_process(root, rustix::process::Signal::KILL);
            } else {
                (self.shell_signal)(rustix::process::Signal::KILL);
            }
        }
        let members = self
            .inspector
            .snapshot()
            .map(|s| self.adopt_descendants(&s))
            .unwrap_or_else(|_| lock(&self.descendants).clone());
        self.signal_members(&members, rustix::process::Signal::KILL);
        if let Some(owner) = &self.managed {
            owner.terminate_for_host_exit();
        }
    }
    fn cleanup_managed_owner(&self) {
        if let Some(owner) = &self.managed
            && !self.managed_owner_cleaned.swap(true, Ordering::AcqRel)
        {
            let _ = owner.cleanup();
        }
    }
    async fn close_once(self: Arc<Self>) -> Result<(), BoxError> {
        self.cleanup_requested.store(true, Ordering::Release);
        let result = self.clone().close_inner().await;
        if self.exited.load(Ordering::Acquire) {
            let target = self.clone();
            tokio::task::spawn_blocking(move || target.cleanup_managed_owner()).await?;
        }
        result
    }
    async fn close_inner(self: Arc<Self>) -> Result<(), BoxError> {
        let _operations = self.operations.clone().write_owned().await;
        self.output.resume_for_cleanup();
        if let Some(owner) = &self.managed {
            let target = owner.clone();
            tokio::task::spawn_blocking(move || target.signal(rustix::process::Signal::TERM))
                .await?;
            let observation = owner.wait_for_exit();
            let result = tokio::select! {result=observation.clone()=>Some(result),_=tokio::time::sleep(self.grace)=>None};
            match result {
                Some(Ok(())) => {}
                None => {
                    let target = owner.clone();
                    tokio::task::spawn_blocking(move || {
                        target.signal(rustix::process::Signal::KILL)
                    })
                    .await?;
                    observation.await?;
                }
                Some(Err(first)) => {
                    let target = owner.clone();
                    tokio::task::spawn_blocking(move || {
                        target.signal(rustix::process::Signal::KILL)
                    })
                    .await?;
                    if let Err(second) = owner.wait_for_exit().await {
                        return Err(Box::new(crate::LocalError::ManagedTeardown(vec![
                            first, second,
                        ])));
                    }
                    return Err(first.into());
                }
            }
            if self.running() {
                tokio::select! {_=self.done.clone()=>{},_=tokio::time::sleep(self.grace)=>{}}
            }
            if self.running() {
                return Err(error(format!(
                    "terminal cleanup failed; surviving pid: {}",
                    self.pid
                )));
            }
        } else {
            let survivors = self.stop_descendants().await?;
            if !survivors.is_empty() {
                return Err(error(format!(
                    "terminal cleanup failed; surviving pids: {}",
                    survivors
                        .iter()
                        .map(|p| p.pid.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            self.stop_shell().await?;
            let survivors = self.stop_descendants().await?;
            if !survivors.is_empty() {
                return Err(error(format!(
                    "terminal cleanup failed; surviving pids: {}",
                    survivors
                        .iter()
                        .map(|p| p.pid.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        self.writer_done.clone().await?;
        self.reader_done.clone().await?;
        lock(&self.state).quiescent = true;
        if let Some(activity) = &self.activity {
            activity.dispose();
        }
        if let Some(callback) = lock(&self.on_quiescence).take() {
            callback();
        }
        Ok(())
    }
    pub fn terminate_shared(self: &Arc<Self>) -> RangeExit {
        let mut cleanup = lock(&self.cleanup);
        if let Some(future) = &*cleanup {
            return future.clone();
        }
        let handle = self.clone();
        let future = async move {
            let result = handle.clone().close_once().await.map_err(SharedError::new);
            if result.is_err() {
                *lock(&handle.cleanup) = None;
            }
            result
        }
        .boxed()
        .shared();
        *cleanup = Some(future.clone());
        future
    }
}
struct TerminalControl(Arc<LocalTerminalHandle>);
impl SubprocessTerminalControl for TerminalControl {
    fn write<'a>(&'a self, data: &'a str) -> SubprocessFuture<'a, ()> {
        let handle = self.0.clone();
        let data = data.to_owned();
        async move {
            let _permit = handle.operations.clone().read_owned().await;
            if !handle.running() {
                return Err(error("terminal process has exited"));
            }
            handle.invalidate();
            handle
                .input
                .send(data.into_bytes())
                .map_err(|_| error("terminal process has exited"))
        }
        .boxed()
    }
    fn resize(&self, cols: u64, rows: u64) -> SubprocessFuture<'_, ()> {
        let handle = self.0.clone();
        async move {
            let permit = handle.operations.clone().read_owned().await;
            if !handle.running() {
                return Err(error("terminal process has exited"));
            }
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                handle.master.resize(
                    u16::try_from(cols).map_err(|_| error("terminal columns are out of range"))?,
                    u16::try_from(rows).map_err(|_| error("terminal rows are out of range"))?,
                )
            })
            .await?
        }
        .boxed()
    }
    fn inspect_foreground(&self) -> SubprocessFuture<'_, Option<SubprocessTerminalForeground>> {
        let handle = self.0.clone();
        async move {
            let permit = handle.operations.clone().read_owned().await;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                handle.foreground()
            })
            .await?
        }
        .boxed()
    }
    fn inspect_activity(&self) -> SubprocessFuture<'_, SubprocessTerminalActivity> {
        let handle = self.0.clone();
        async move {
            let permit = handle.operations.clone().read_owned().await;
            Ok(tokio::task::spawn_blocking(move || {
                let _permit = permit;
                handle.activity_now()
            })
            .await?)
        }
        .boxed()
    }
    fn signal_foreground(&self, signal: SubprocessTerminalSignal) -> SubprocessFuture<'_, u64> {
        let handle = self.0.clone();
        async move{let permit=handle.operations.clone().read_owned().await;tokio::task::spawn_blocking(move||{let _permit=permit;handle.invalidate();let foreground=handle.foreground()?.ok_or_else(||error(format!("cannot resolve foreground process group for terminal {}",handle.pid)))?;if signal==SubprocessTerminalSignal::Sigkill&&foreground.process_group_id==u64::from(handle.pid){return Err(error("refusing to SIGKILL the terminal shell; terminate the terminal session instead"));}handle.inspector.signal_group(u32::try_from(foreground.process_group_id)?,signal)?;Ok(foreground.process_group_id)}).await?}.boxed()
    }
    fn terminate(&self) -> SubprocessFuture<'_, ()> {
        let pending = self.0.terminate_shared();
        async move { pending.await.map_err(Into::into) }.boxed()
    }
}

type PreparedTerminal = (
    Box<std::process::Child>,
    Box<dyn MasterPty + Send>,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Option<Arc<ShellActivity>>,
    Option<crate::linux_scope::LinuxScopeStartup>,
    Option<(LinuxScopeInternals, Vec<String>)>,
    Option<String>,
);
#[derive(Default)]
struct AllocationState {
    cancelled: bool,
    prepared: Option<Result<PreparedTerminal, BoxError>>,
}
// Constructed and dropped only in a blocking allocation/cleanup worker.
struct PreparingTerminal {
    child: Option<Box<std::process::Child>>,
    activity: Option<Arc<ShellActivity>>,
    startup: Option<crate::linux_scope::LinuxScopeStartup>,
    native: Option<(LinuxScopeInternals, String)>,
    armed: bool,
}
impl Drop for PreparingTerminal {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some((internals, unit)) = &self.native {
            let _ = (internals.run_sync)(
                &internals.systemctl,
                &[
                    "--user".into(),
                    "kill".into(),
                    "--kill-who=all".into(),
                    "--signal=SIGKILL".into(),
                    unit.clone(),
                ],
                &crate::linux_scope::manager_environment(),
            );
        }
        if let Some(child) = &mut self.child {
            crate::spawn::kill_group(Some(child.id()), rustix::process::Signal::KILL);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(startup) = &self.startup {
            cleanup_linux_launch_files(&startup.files);
        }
        if let Some(activity) = &self.activity {
            activity.dispose();
        }
    }
}
fn dispose_unbound(prepared: PreparedTerminal) {
    let (child, _master, _reader, _writer, activity, startup, scope, unit) = prepared;
    drop(PreparingTerminal {
        child: Some(child),
        activity,
        startup,
        native: scope
            .zip(unit)
            .map(|((internals, _), unit)| (internals, unit)),
        armed: true,
    });
}

pub async fn spawn_terminal(
    ctx: &Context,
    spec: SubprocessTerminalSpawnSpec,
    inspector: Arc<dyn ProcessInspector>,
    scope: Option<(LinuxScopeInternals, Vec<String>)>,
) -> Result<(SubprocessTerminalHandle, Arc<LocalTerminalHandle>), BoxError> {
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
    let ordinary = SubprocessSpawnSpec {
        argv: spec.argv.clone(),
        cwd: spec.cwd.clone(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Inherit,
            stderr: SubprocessOutputMode::Inherit,
            control: None,
        },
        grace_ms: spec.grace_ms,
        signal: None,
        env: spec.env.as_ref().map(|env| {
            env.iter()
                .map(|(k, v)| (k.clone(), Some(v.clone())))
                .collect()
        }),
    };
    let env = target_environment(&ordinary)?;
    let allocation = Arc::new(Mutex::new(AllocationState::default()));
    let (prepared_send, prepared_receive) = tokio::sync::oneshot::channel();
    let prepared_ready = async move { prepared_receive.await.map_err(SharedError::new) }
        .boxed()
        .shared();
    let abandoned = allocation.clone();
    ctx.effect("terminal allocation", move || {
        Undo::future(move || async move {
            lock(&abandoned).cancelled = true;
            prepared_ready.await?;
            let prepared = lock(&abandoned).prepared.take();
            if let Some(Ok(prepared)) = prepared {
                tokio::task::spawn_blocking(move || dispose_unbound(prepared)).await?;
            }
            Ok(())
        })
    })?;
    let capture = spec.clone();
    let published = allocation.clone();
    tokio::task::spawn_blocking(move || {
        let result = (|| -> Result<PreparedTerminal, BoxError> {
            let activity = prepare_shell_activity(&capture, env, "linux")?.map(Arc::new);
            let mut preparing = PreparingTerminal {
                child: None,
                activity: activity.clone(),
                startup: None,
                native: None,
                armed: true,
            };
            let mut env = activity
                .as_ref()
                .map_or_else(|| target_environment(&ordinary), |a| Ok(a.env.clone()))?;
            env.insert("TERM".into(), capture.terminal_type.clone());
            let argv = activity
                .as_ref()
                .map_or_else(|| capture.argv.clone(), |a| a.argv.clone());
            let mut startup = None;
            let mut unit = None;
            let (command, args, cwd, environment) = if let Some((internals, literal)) = &scope {
                env.insert("PWD".into(), capture.cwd.clone());
                let mut target = capture.clone();
                target.argv = argv;
                let launch =
                    prepare_linux_terminal_scope(&target, env, literal, internals.clone())?;
                startup = Some(launch.startup);
                unit = Some(launch.unit);
                preparing.startup = startup.clone();
                preparing.native = unit.clone().map(|unit| (internals.clone(), unit));
                (launch.command, launch.args, launch.cwd, launch.env)
            } else {
                (
                    argv[0].clone(),
                    argv[1..].to_vec(),
                    capture.cwd.clone(),
                    env,
                )
            };
            let pair = portable_pty::native_pty_system()
                .openpty(PtySize {
                    rows: u16::try_from(capture.rows)?,
                    cols: u16::try_from(capture.cols)?,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| error(e.to_string()))?;
            let mut builder = CommandBuilder::new(command);
            builder.args(args);
            builder.cwd(cwd);
            builder.env_clear();
            for (key, value) in environment {
                builder.env(key, value);
            }
            let child = match pair.slave.spawn_command(builder) {
                Ok(child) => child,
                Err(error) => {
                    if let Some(startup) = &startup {
                        cleanup_linux_launch_files(&startup.files);
                    }
                    if let Some(activity) = &activity {
                        activity.dispose();
                    }
                    return Err(crate::error(error.to_string()));
                }
            };
            let child: Box<dyn portable_pty::Child> = child;
            let child = child
                .downcast::<std::process::Child>()
                .map_err(|_| error("portable-pty returned an unsupported Linux child handle"))?;
            preparing.child = Some(child);
            drop(pair.slave);
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|e| error(e.to_string()))?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|e| error(e.to_string()))?;
            preparing.armed = false;
            Ok((
                preparing
                    .child
                    .take()
                    .ok_or_else(|| error("terminal child is absent"))?,
                pair.master,
                reader,
                writer,
                activity,
                startup,
                scope,
                unit,
            ))
        })();
        lock(&published).prepared = Some(result);
        let _ = prepared_send.send(());
    })
    .await?;
    let prepared = {
        let mut state = lock(&allocation);
        if state.cancelled {
            return Err(error("terminal allocation cancelled"));
        }
        state
            .prepared
            .take()
            .ok_or_else(|| error("terminal allocation is absent"))??
    };
    let (child, master, mut reader, mut writer, activity, startup, scope, unit) = prepared;
    let pid = child.id();
    let root_identity = inspector
        .snapshot()
        .ok()
        .and_then(|s| s.tree(pid).into_iter().find(|p| p.pid == pid));
    let (send, receive) = tokio::sync::oneshot::channel();
    let done = subprocess_done(async move { receive.await.map_err(SharedError::new)? });
    let (exit_send, exit_receive) = tokio::sync::oneshot::channel();
    let direct_exit = async move {
        let _ = exit_receive.await;
        Ok(())
    }
    .boxed()
    .shared();
    let active = Arc::new(AtomicBool::new(true));
    let managed = scope
        .zip(unit)
        .zip(startup.clone())
        .map(|(((internals, _), unit), startup)| {
            Arc::new(SystemdScopeOwner::new(
                unit,
                startup,
                DirectRange {
                    running: active.clone(),
                    signal: Arc::new(move |signal| {
                        crate::spawn::signal_direct_process(pid, signal)
                    }),
                    settled: direct_exit,
                },
                internals,
            )) as Arc<dyn BoundProcessOwner>
        });
    let output = OutputQueue::new();
    let (reader_send, reader_receive) = tokio::sync::oneshot::channel();
    let reader_done = async move {
        reader_receive.await.map_err(SharedError::new)?;
        Ok(())
    }
    .boxed()
    .shared();
    let queue = output.clone();
    let reader_task = tokio::task::spawn_blocking(move || {
        let mut pending = vec![];
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    let final_bytes = decode_utf8(&mut pending, &[], true);
                    queue.push(final_bytes);
                    break;
                }
                Ok(count) => {
                    if !queue.push(decode_utf8(&mut pending, &buffer[..count], false)) {
                        break;
                    }
                }
            }
        }
        queue.end();
        let _ = reader_send.send(());
    });
    let (input, input_receive) = std::sync::mpsc::channel::<Vec<u8>>();
    let (writer_send, writer_receive) = tokio::sync::oneshot::channel();
    let writer_done = async move {
        writer_receive.await.map_err(SharedError::new)?;
        Ok(())
    }
    .boxed()
    .shared();
    let writer_queue = output.clone();
    let writer_task = tokio::task::spawn_blocking(move || {
        loop {
            match input_receive.recv_timeout(Duration::from_millis(15)) {
                Ok(data) => {
                    if writer_queue.terminating.load(Ordering::Acquire) {
                        break;
                    }
                    if writer.write_all(&data).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if writer_queue.terminating.load(Ordering::Acquire) {
                        break;
                    }
                }
            }
        }
        let _ = writer_send.send(());
    });
    let handle = Arc::new(LocalTerminalHandle {
        pid,
        done: done.clone(),
        master: Arc::new(PortableMaster(Mutex::new(master))),
        input,
        output: output.clone(),
        inspector,
        root_identity,
        descendants: Mutex::default(),
        exited: AtomicBool::new(false),
        grace: crate::spawn::grace(spec.grace_ms),
        managed,
        activity,
        state: Mutex::new(ActivityState {
            key: String::new(),
            revision: 0,
            quiescent: false,
            range_empty: false,
        }),
        cleanup: Mutex::new(None),
        reader_done,
        writer_done,
        operations: Arc::new(tokio::sync::RwLock::new(())),
        on_quiescence: Mutex::default(),
        shell_signal: Arc::new(move |signal| crate::spawn::signal_direct_process(pid, signal)),
        cleanup_requested: AtomicBool::new(false),
        managed_owner_cleaned: AtomicBool::new(false),
    });
    let instance = handle.clone();
    let child = Arc::new(Mutex::new(Some(child)));
    let waiting_child = child.clone();
    let direct_registered = ctx.spawn("terminal direct exit", move |_cancel| async move {
        let result = tokio::task::spawn_blocking(move || {
            let mut child = lock(&waiting_child)
                .take()
                .ok_or_else(|| std::io::Error::other("terminal child is absent"))?;
            child.wait()
        })
        .await?;
        active.store(false, Ordering::Release);
        instance.exited.store(true, Ordering::Release);
        let _ = exit_send.send(());
        let result = result.map(crate::spawn::outcome).map_err(SharedError::new);
        let result = if let Some(startup) = startup {
            match result {
                Ok(outcome) => {
                    tokio::task::spawn_blocking(move || startup.resolve_outcome(outcome))
                        .await
                        .map_err(SharedError::new)?
                        .map_err(SharedError::new)
                }
                Err(error) => Err(error),
            }
        } else {
            result
        };
        let _ = send.send(result);
        if instance.cleanup_requested.load(Ordering::Acquire) {
            tokio::task::spawn_blocking(move || instance.cleanup_managed_owner()).await?;
        }
        Ok(())
    });
    if let Err(error) = direct_registered {
        let prepared_handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            prepared_handle.output.resume_for_cleanup();
            if let Some(owner) = &prepared_handle.managed {
                owner.terminate_for_host_exit();
            }
            if let Some(mut child) = lock(&child).take() {
                crate::spawn::kill_group(Some(child.id()), rustix::process::Signal::KILL);
                let _ = child.kill();
                let _ = child.wait();
            }
            prepared_handle.exited.store(true, Ordering::Release);
            prepared_handle.cleanup_managed_owner();
            if let Some(activity) = &prepared_handle.activity {
                activity.dispose();
            }
        })
        .await?;
        return Err(error.into());
    }
    let instance = handle.clone();
    let observe_shell_exit = spec.shell_activity == Some(true);
    let registered = (|| {
        ctx.spawn("terminal ownership",move|cancel|async move{tokio::select!{_=cancel.cancelled()=>{instance.terminate_shared().await?;},_=instance.done.clone()=>{if !observe_shell_exit{instance.terminate_shared().await?;}else if let Some(owner)=&instance.managed{tokio::select!{_=cancel.cancelled()=>{instance.terminate_shared().await?;},result=owner.wait_for_exit()=>{if result.is_ok(){lock(&instance.state).range_empty=true;}}}}else{cancel.cancelled().await;instance.terminate_shared().await?;}}}Ok(())})?;
        ctx.spawn("terminal reader", move |_cancel| async move {
            reader_task.await?;
            Ok(())
        })?;
        ctx.spawn("terminal writer", move |_cancel| async move {
            writer_task.await?;
            Ok(())
        })?;
        Ok::<_, hoh_kernel::KernelError>(())
    })();
    if let Err(error) = registered {
        let _ = handle.terminate_shared().await;
        return Err(error.into());
    }
    Ok((
        SubprocessTerminalHandle {
            pid: u64::from(pid),
            output: Box::new(TerminalOutput(output)),
            done,
            lifecycle: Arc::new(TerminalControl(handle.clone())),
        },
        handle,
    ))
}
