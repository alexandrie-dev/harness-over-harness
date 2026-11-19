use super::*;
use crate::process_inspector::ProcStat;
use std::sync::{Weak, atomic::AtomicUsize};
use tokio::io::AsyncReadExt;
type ExitCell =
    Arc<Mutex<Option<tokio::sync::oneshot::Sender<Result<SubprocessOutcome, SharedError>>>>>;

pub(crate) fn retained_terminal(fails: bool) -> Arc<LocalTerminalHandle> {
    let inspector = Inspector::new();
    lock(&inspector.0).ignore_shell_signals = fails;
    Fixture::new(inspector, None).handle
}
struct MemoryMaster(Mutex<(u16, u16)>);
impl TerminalMaster for MemoryMaster {
    fn resize(&self, cols: u16, rows: u16) -> Result<(), BoxError> {
        *lock(&self.0) = (cols, rows);
        Ok(())
    }
    fn dimensions(&self) -> (u16, u16) {
        *lock(&self.0)
    }
}

struct World {
    rows: Vec<ProcStat>,
    complete: bool,
    read_error: bool,
    pgid: Option<u32>,
    waiting: bool,
    remove_on_signal: bool,
    ignore_shell_signals: bool,
    kills: Vec<(u32, rustix::process::Signal)>,
    groups: Vec<(u32, SubprocessTerminalSignal)>,
    root_signal: Option<Arc<dyn Fn() + Send + Sync>>,
}
struct Inspector(Mutex<World>);
fn row(pid: u32, parent: u32, session: i32, started: &str) -> ProcStat {
    ProcStat {
        pid,
        parent_pid: parent,
        pgrp: pid as i32,
        session,
        state: "S".into(),
        tty_device: 99,
        tpgid: 123,
        started: started.into(),
    }
}
impl Inspector {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(World {
            rows: vec![row(123, 1, 123, "shell")],
            complete: true,
            read_error: false,
            pgid: Some(456),
            waiting: false,
            remove_on_signal: true,
            ignore_shell_signals: false,
            kills: vec![],
            groups: vec![],
            root_signal: None,
        })))
    }
}
impl ProcessInspector for Inspector {
    fn foreground_pgid(&self, _: u32) -> Option<u32> {
        lock(&self.0).pgid
    }
    fn is_stdin_waiting(&self, _: u32, _: u32) -> bool {
        lock(&self.0).waiting
    }
    fn snapshot(&self) -> Result<ProcessSnapshot, BoxError> {
        let world = lock(&self.0);
        if world.read_error {
            return Err(error(
                "Cannot inspect processes: /proc directory is unreadable",
            ));
        }
        Ok(ProcessSnapshot::from_rows(
            world.rows.clone(),
            world.complete,
        ))
    }
    fn is_alive(&self, identity: &ProcessIdentity) -> bool {
        lock(&self.0).rows.iter().any(|row| {
            row.pid == identity.pid
                && row.started == identity.started
                && !crate::process_inspector::quiescent(&row.state)
        })
    }
    fn signal_group(&self, pid: u32, signal: SubprocessTerminalSignal) -> Result<(), BoxError> {
        lock(&self.0).groups.push((pid, signal));
        Ok(())
    }
    fn signal_process(
        &self,
        identity: &ProcessIdentity,
        signal: rustix::process::Signal,
    ) -> Result<(), BoxError> {
        if !self.is_alive(identity) {
            return Ok(());
        }
        let callback = {
            let mut world = lock(&self.0);
            world.kills.push((identity.pid, signal));
            if world.remove_on_signal {
                world.rows.retain(|row| row.pid != identity.pid);
            }
            if identity.pid == 123 {
                world.root_signal.clone()
            } else {
                None
            }
        };
        if let Some(callback) = callback {
            callback();
        }
        Ok(())
    }
}
struct Fixture {
    handle: Arc<LocalTerminalHandle>,
    inspector: Arc<Inspector>,
    exit: ExitCell,
    _input: std::sync::mpsc::Receiver<Vec<u8>>,
    _directory: tempfile::TempDir,
}
impl Fixture {
    fn new(inspector: Arc<Inspector>, managed: Option<Arc<dyn BoundProcessOwner>>) -> Self {
        let master = Arc::new(MemoryMaster(Mutex::new((80, 24))));
        let (send, receive) = tokio::sync::oneshot::channel();
        let exit = Arc::new(Mutex::new(Some(send)));
        let done = subprocess_done(async move { receive.await.map_err(SharedError::new)? });
        let (input, receiver) = std::sync::mpsc::channel();
        let directory = tempfile::tempdir().unwrap();
        let activity = Arc::new(ShellActivity::new(
            directory.path().into(),
            vec![],
            Default::default(),
        ));
        std::fs::write(directory.path().join("state"), "123:1:idle\n").unwrap();
        let target: Arc<Mutex<Weak<LocalTerminalHandle>>> = Arc::default();
        let signal_target = target.clone();
        let signal_exit = exit.clone();
        let root_identity = inspector
            .snapshot()
            .ok()
            .and_then(|s| s.tree(123).into_iter().find(|p| p.pid == 123));
        let world = inspector.clone();
        let handle = Arc::new(LocalTerminalHandle {
            pid: 123,
            done,
            master,
            input,
            output: OutputQueue::new(),
            inspector: inspector.clone(),
            root_identity,
            descendants: Mutex::default(),
            exited: AtomicBool::new(false),
            grace: Duration::from_millis(10),
            managed,
            activity: Some(activity),
            state: Mutex::new(ActivityState {
                key: String::new(),
                revision: 0,
                quiescent: false,
                range_empty: false,
            }),
            cleanup: Mutex::default(),
            reader_done: async { Ok(()) }.boxed().shared(),
            writer_done: async { Ok(()) }.boxed().shared(),
            operations: Arc::new(tokio::sync::RwLock::new(())),
            on_quiescence: Mutex::default(),
            shell_signal: Arc::new(move |signal| {
                if lock(&world.0).ignore_shell_signals {
                    return false;
                }
                if let Some(handle) = lock(&signal_target).upgrade() {
                    handle.exited.store(true, Ordering::Release);
                    handle.output.end();
                    if let Some(exit) = lock(&signal_exit).take() {
                        let _ = exit.send(Ok(SubprocessOutcome {
                            exit_code: None,
                            signal: crate::spawn::signal_name(signal.as_raw()).map(Into::into),
                        }));
                    }
                }
                true
            }),
            cleanup_requested: AtomicBool::new(false),
            managed_owner_cleaned: AtomicBool::new(false),
        });
        *lock(&target) = Arc::downgrade(&handle);
        let weak = Arc::downgrade(&handle);
        let emit = exit.clone();
        lock(&inspector.0).root_signal = Some(Arc::new(move || {
            if let Some(handle) = weak.upgrade() {
                handle.exited.store(true, Ordering::Release);
                handle.output.end();
                if let Some(exit) = lock(&emit).take() {
                    let _ = exit.send(Ok(SubprocessOutcome {
                        exit_code: None,
                        signal: Some("SIGKILL".into()),
                    }));
                }
            }
        }));
        Self {
            handle,
            inspector,
            exit,
            _input: receiver,
            _directory: directory,
        }
    }
    fn emit_exit(&self, outcome: SubprocessOutcome) {
        self.handle.exited.store(true, Ordering::Release);
        self.handle.output.end();
        if let Some(exit) = lock(&self.exit).take() {
            let _ = exit.send(Ok(outcome));
        }
        if self.handle.cleanup_requested.load(Ordering::Acquire) {
            self.handle.cleanup_managed_owner();
        }
    }
    fn control(&self) -> TerminalControl {
        TerminalControl(self.handle.clone())
    }
}
#[tokio::test]
async fn bridges_terminal_bytes_foreground_control_and_signalled_exit_facts() {
    let inspector = Inspector::new();
    lock(&inspector.0).waiting = true;
    let fixture = Fixture::new(inspector, None);
    let control = fixture.control();
    fixture.handle.output.push("hello €".as_bytes().to_vec());
    control.write("input\r").await.unwrap();
    assert_eq!(fixture._input.try_recv().unwrap(), b"input\r");
    control.resize(100, 30).await.unwrap();
    assert_eq!(fixture.handle.master.dimensions(), (100, 30));
    let foreground = control.inspect_foreground().await.unwrap().unwrap();
    assert_eq!(
        (foreground.process_group_id, foreground.input_waiting),
        (456, true)
    );
    assert_eq!(
        control
            .signal_foreground(SubprocessTerminalSignal::Sigint)
            .await
            .unwrap(),
        456
    );
    assert_eq!(
        lock(&fixture.inspector.0).groups,
        [(456, SubprocessTerminalSignal::Sigint)]
    );
    fixture.emit_exit(SubprocessOutcome {
        exit_code: None,
        signal: Some("SIGKILL".into()),
    });
    assert!(
        control
            .resize(80, 24)
            .await
            .unwrap_err()
            .to_string()
            .contains("has exited")
    );
    assert_eq!(
        fixture.handle.done.clone().await.unwrap().signal.as_deref(),
        Some("SIGKILL")
    );
    fixture.handle.terminate_shared().await.unwrap();
    let mut bytes = vec![];
    TerminalOutput(fixture.handle.output.clone())
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    assert_eq!(bytes, "hello €".as_bytes());
}
#[tokio::test]
async fn rejects_unsafe_foreground_signals_and_writes_after_exit() {
    let inspector = Inspector::new();
    let fixture = Fixture::new(inspector.clone(), None);
    let control = fixture.control();
    lock(&inspector.0).pgid = Some(123);
    assert!(
        control
            .signal_foreground(SubprocessTerminalSignal::Sigkill)
            .await
            .unwrap_err()
            .to_string()
            .contains("terminate the terminal session")
    );
    lock(&inspector.0).pgid = None;
    assert!(control.inspect_foreground().await.unwrap().is_none());
    assert!(
        control
            .signal_foreground(SubprocessTerminalSignal::Sigterm)
            .await
            .unwrap_err()
            .to_string()
            .contains("cannot resolve")
    );
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(3),
        signal: None,
    });
    fixture.handle.terminate_shared().await.unwrap();
    assert!(
        control
            .write("late")
            .await
            .unwrap_err()
            .to_string()
            .contains("has exited")
    );
}
#[tokio::test]
async fn retains_ownership_after_shell_exit_when_proc_cannot_be_enumerated() {
    let inspector = Inspector::new();
    lock(&inspector.0).read_error = true;
    let fixture = Fixture::new(inspector.clone(), None);
    let released = Arc::new(AtomicUsize::new(0));
    let record = released.clone();
    fixture.handle.on_quiescence(Arc::new(move || {
        record.fetch_add(1, Ordering::SeqCst);
    }));
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    assert_eq!(
        fixture.control().inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    assert!(
        fixture
            .handle
            .terminate_shared()
            .await
            .unwrap_err()
            .to_string()
            .contains("/proc directory is unreadable")
    );
    assert_eq!(released.load(Ordering::SeqCst), 0);
    {
        let mut world = lock(&inspector.0);
        world.read_error = false;
        world.rows.clear();
    }
    assert_eq!(
        fixture.control().inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Idle
    );
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn requires_matching_shell_identity_complete_process_observations_and_an_idle_foreground_before_reporting_idle()
 {
    let inspector = Inspector::new();
    lock(&inspector.0).pgid = Some(123);
    let fixture = Fixture::new(inspector.clone(), None);
    let control = fixture.control();
    let idle = control.inspect_activity().await.unwrap();
    assert_eq!(idle.state, SubprocessTerminalActivityState::Idle);
    assert_eq!(control.inspect_activity().await.unwrap(), idle);
    control.write("partial").await.unwrap();
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    std::fs::write(fixture._directory.path().join("state"), "123:2:idle\n").unwrap();
    lock(&inspector.0).complete = false;
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    lock(&inspector.0).complete = true;
    lock(&inspector.0).pgid = None;
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    lock(&inspector.0).pgid = Some(456);
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Busy
    );
    lock(&inspector.0).pgid = Some(123);
    lock(&inspector.0).rows[0].started = "reused".into();
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    lock(&inspector.0).rows[0].started = "shell".into();
    lock(&inspector.0)
        .rows
        .push(row(789, 123, 123, "background"));
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Busy
    );
    lock(&inspector.0).rows.retain(|row| row.pid == 123);
    lock(&inspector.0).read_error = true;
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    lock(&inspector.0).read_error = false;
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(
        control.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Idle
    );
}
#[tokio::test]
async fn keeps_unverified_root_identities_unknown_without_leaking_a_failed_pty_allocation() {
    let inspector = Inspector::new();
    lock(&inspector.0).pgid = Some(123);
    lock(&inspector.0).read_error = true;
    let fixture = Fixture::new(inspector.clone(), None);
    lock(&inspector.0).read_error = false;
    assert_eq!(
        fixture.control().inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Unknown
    );
    fixture.handle.terminate_shared().await.unwrap();
}
#[tokio::test]
async fn does_not_adopt_the_children_of_a_recycled_shell_pid() {
    let inspector = Inspector::new();
    let fixture = Fixture::new(inspector.clone(), None);
    {
        let mut world = lock(&inspector.0);
        world.rows[0].started = "reused".into();
        world.rows.push(row(789, 123, 123, "unrelated"));
    }
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    fixture.handle.terminate_shared().await.unwrap();
    assert!(lock(&inspector.0).kills.is_empty());
}
#[tokio::test]
async fn adopts_nothing_when_the_shell_identity_was_never_observable() {
    let inspector = Inspector::new();
    lock(&inspector.0).rows.clear();
    let fixture = Fixture::new(inspector.clone(), None);
    {
        let mut world = lock(&inspector.0);
        world.rows.push(row(123, 1, 123, "reused"));
        world.rows.push(row(789, 123, 123, "unrelated"));
    }
    fixture.handle.terminate_shared().await.unwrap();
    assert!(lock(&inspector.0).kills.is_empty());
}
#[tokio::test]
async fn retains_an_inspected_descendant_after_it_reparents_away_from_the_shell() {
    let inspector = Inspector::new();
    lock(&inspector.0).rows.push(row(789, 123, 123, "child"));
    let fixture = Fixture::new(inspector.clone(), None);
    fixture.control().inspect_foreground().await.unwrap();
    {
        let mut world = lock(&inspector.0);
        world.rows.retain(|row| row.pid != 123);
        world.rows[0].parent_pid = 1;
        world.rows[0].session = 789;
    }
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(
        lock(&inspector.0).kills,
        [(789, rustix::process::Signal::TERM)]
    );
}
#[tokio::test]
async fn reports_a_top_level_process_that_ignores_escalation() {
    let inspector = Inspector::new();
    lock(&inspector.0).ignore_shell_signals = true;
    let fixture = Fixture::new(inspector, None);
    assert_eq!(
        fixture
            .handle
            .terminate_shared()
            .await
            .unwrap_err()
            .to_string(),
        "terminal cleanup failed; surviving pid: 123"
    );
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    fixture.handle.terminate_shared().await.unwrap();
}
#[test]
fn terminal_utf8_decode_preserves_split_codepoints_and_replaces_incomplete_tail() {
    let mut pending = vec![];
    assert_eq!(decode_utf8(&mut pending, &[0xe2], false), b"");
    assert_eq!(
        decode_utf8(&mut pending, &[0x82, 0xac], false),
        "€".as_bytes()
    );
    assert_eq!(
        decode_utf8(&mut pending, &[0xf0, 0x9f], true),
        "�".as_bytes()
    );
}

struct Owner {
    signals: Mutex<Vec<rustix::process::Signal>>,
    results: Mutex<Vec<Result<(), SharedError>>>,
    stop_on: Option<rustix::process::Signal>,
    stopped: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    terminal: Mutex<Weak<LocalTerminalHandle>>,
    exit: Mutex<Option<ExitCell>>,
    cleanups: AtomicUsize,
    host_kills: AtomicUsize,
    tasks: Option<u64>,
}
impl Owner {
    fn new(
        stop_on: Option<rustix::process::Signal>,
        results: Vec<Result<(), SharedError>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            signals: Mutex::default(),
            results: Mutex::new(results),
            stop_on,
            stopped: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(tokio::sync::Notify::new()),
            terminal: Mutex::default(),
            exit: Mutex::default(),
            cleanups: AtomicUsize::new(0),
            host_kills: AtomicUsize::new(0),
            tasks: None,
        })
    }
    fn connect(&self, fixture: &Fixture) {
        *lock(&self.terminal) = Arc::downgrade(&fixture.handle);
        *lock(&self.exit) = Some(fixture.exit.clone());
    }
}
impl BoundProcessOwner for Owner {
    fn signal(&self, signal: rustix::process::Signal) {
        lock(&self.signals).push(signal);
        if self.stop_on == Some(signal) {
            self.stopped.store(true, Ordering::Release);
            self.wake.notify_waiters();
            if let Some(terminal) = lock(&self.terminal).upgrade() {
                terminal.exited.store(true, Ordering::Release);
                terminal.output.end();
            }
            if let Some(exit) = lock(&self.exit).as_ref()
                && let Some(exit) = lock(exit).take()
            {
                let _ = exit.send(Ok(SubprocessOutcome {
                    exit_code: None,
                    signal: crate::spawn::signal_name(signal.as_raw()).map(Into::into),
                }));
            }
        }
    }
    fn wait_for_exit(&self) -> RangeExit {
        if !lock(&self.results).is_empty() {
            return futures_util::future::ready(lock(&self.results).remove(0))
                .boxed()
                .shared();
        }
        let stopped = self.stopped.clone();
        let wake = self.wake.clone();
        async move {
            loop {
                let pending = wake.notified();
                tokio::pin!(pending);
                pending.as_mut().enable();
                if stopped.load(Ordering::Acquire) {
                    return Ok(());
                }
                pending.await;
            }
        }
        .boxed()
        .shared()
    }
    fn inspect_task_count(&self) -> Result<Option<u64>, BoxError> {
        Ok(self.tasks)
    }
    fn terminate_for_host_exit(&self) {
        self.host_kills.fetch_add(1, Ordering::SeqCst);
    }
    fn cleanup(&self) -> Result<(), BoxError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn terminates_a_managed_range_with_term_when_it_stops_within_the_grace_period() {
    let owner = Owner::new(Some(rustix::process::Signal::TERM), vec![]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(*lock(&owner.signals), [rustix::process::Signal::TERM]);
    assert_eq!(
        fixture.handle.done.clone().await.unwrap().signal.as_deref(),
        Some("SIGTERM")
    );
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 1);
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn escalates_a_managed_range_to_kill_after_the_term_grace_expires() {
    let owner = Owner::new(Some(rustix::process::Signal::KILL), vec![]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(
        *lock(&owner.signals),
        [rustix::process::Signal::TERM, rustix::process::Signal::KILL]
    );
}
#[tokio::test]
async fn force_kills_and_retries_a_managed_range_when_observation_first_rejects() {
    let failure = SharedError::new(error("scope became unreadable"));
    let owner = Owner::new(None, vec![Err(failure.clone()), Ok(())]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    let error = fixture.handle.terminate_shared().await.unwrap_err();
    assert!(error.ptr_eq(&failure));
    assert_eq!(
        *lock(&owner.signals),
        [rustix::process::Signal::TERM, rustix::process::Signal::KILL]
    );
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 0);
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
}
#[tokio::test]
async fn preserves_both_failed_managed_range_observations_after_force_kill() {
    let first = SharedError::new(error("scope became unreadable"));
    let second = SharedError::new(error("scope stayed unreadable"));
    let owner = Owner::new(None, vec![Err(first.clone()), Err(second.clone())]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    let error = fixture.handle.terminate_shared().await.unwrap_err();
    assert_eq!(error.to_string(), "terminal managed-range cleanup failed");
    let crate::LocalError::ManagedTeardown(errors) =
        error.downcast_ref::<crate::LocalError>().unwrap()
    else {
        panic!("wrong cleanup error")
    };
    assert!(errors[0].ptr_eq(&first));
    assert!(errors[1].ptr_eq(&second));
    assert_eq!(
        *lock(&owner.signals),
        [rustix::process::Signal::TERM, rustix::process::Signal::KILL]
    );
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
}
#[tokio::test]
async fn rejects_when_a_managed_range_stops_but_pty_never_publishes_exit() {
    let owner = Owner::new(None, vec![Ok(())]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    assert_eq!(
        fixture
            .handle
            .terminate_shared()
            .await
            .unwrap_err()
            .to_string(),
        "terminal cleanup failed; surviving pid: 123"
    );
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
}
#[test]
fn routes_managed_terminal_host_exit_directly_to_kill() {
    let inspector = Inspector::new();
    inspector.0.lock().unwrap().rows.clear();
    let owner = Owner::new(None, vec![Ok(())]);
    let fixture = Fixture::new(inspector, Some(owner.clone()));
    owner.connect(&fixture);
    fixture.handle.terminate_for_host_exit();
    assert!(lock(&owner.signals).is_empty());
    assert_eq!(owner.host_kills.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn checks_native_range_task_count_before_trusting_a_prompt_with_no_visible_children() {
    for tasks in [None, Some(0), Some(1), Some(2)] {
        let mut owner = Owner::new(Some(rustix::process::Signal::TERM), vec![]);
        Arc::get_mut(&mut owner).unwrap().tasks = tasks;
        let inspector = Inspector::new();
        lock(&inspector.0).pgid = Some(123);
        let fixture = Fixture::new(inspector, Some(owner.clone()));
        owner.connect(&fixture);
        assert_eq!(
            fixture.control().inspect_activity().await.unwrap().state,
            match tasks {
                Some(1) => SubprocessTerminalActivityState::Idle,
                Some(2) => SubprocessTerminalActivityState::Busy,
                _ => SubprocessTerminalActivityState::Unknown,
            }
        );
        fixture.handle.terminate_shared().await.unwrap();
    }
}
#[tokio::test]
async fn cleans_a_same_session_descendant_after_the_top_level_shell_exits_naturally() {
    let inspector = Inspector::new();
    lock(&inspector.0).rows.push(row(789, 1, 123, "orphan"));
    let fixture = Fixture::new(inspector.clone(), None);
    fixture.control().inspect_foreground().await.unwrap();
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    lock(&inspector.0).rows.retain(|row| row.pid != 123);
    fixture.handle.terminate_shared().await.unwrap();
    assert_eq!(
        lock(&inspector.0).kills,
        [(789, rustix::process::Signal::TERM)]
    );
}
#[tokio::test]
async fn retries_failed_cleanup_after_a_surviving_descendant_leaves() {
    let inspector = Inspector::new();
    {
        let mut world = lock(&inspector.0);
        world.rows.push(row(789, 123, 123, "child"));
        world.remove_on_signal = false;
    }
    let fixture = Fixture::new(inspector.clone(), None);
    assert_eq!(
        fixture
            .handle
            .terminate_shared()
            .await
            .unwrap_err()
            .to_string(),
        "terminal cleanup failed; surviving pids: 789"
    );
    assert!(fixture.handle.running());
    lock(&inspector.0).rows.retain(|row| row.pid != 789);
    fixture.handle.terminate_shared().await.unwrap();
    assert!(!fixture.handle.running());
}
#[tokio::test]
async fn force_kills_descendants_around_the_shell_during_synchronous_host_exit() {
    let inspector = Inspector::new();
    lock(&inspector.0).rows.push(row(789, 123, 123, "child"));
    let fixture = Fixture::new(inspector.clone(), None);
    fixture.handle.terminate_for_host_exit();
    assert_eq!(
        lock(&inspector.0).kills,
        [
            (789, rustix::process::Signal::KILL),
            (123, rustix::process::Signal::KILL)
        ]
    );
    assert!(!fixture.handle.running());
}
#[tokio::test]
async fn uses_captured_identities_when_final_inspection_fails() {
    let inspector = Inspector::new();
    lock(&inspector.0).rows.push(row(789, 123, 123, "child"));
    let fixture = Fixture::new(inspector.clone(), None);
    fixture.control().inspect_foreground().await.unwrap();
    lock(&inspector.0).read_error = true;
    fixture.handle.terminate_for_host_exit();
    assert_eq!(
        lock(&inspector.0).kills,
        [
            (789, rustix::process::Signal::KILL),
            (123, rustix::process::Signal::KILL)
        ]
    );
}
#[tokio::test]
async fn does_not_signal_a_recycled_terminal_root_before_its_delayed_exit_callback() {
    let inspector = Inspector::new();
    let fixture = Fixture::new(inspector.clone(), None);
    lock(&inspector.0).rows[0].started = "recycled".into();
    fixture.handle.terminate_for_host_exit();
    assert!(lock(&inspector.0).kills.is_empty());
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    fixture.handle.terminate_shared().await.unwrap();
}

#[tokio::test]
async fn pauses_native_output_until_the_consumer_drains_and_resumes_before_termination() {
    let fixture = Fixture::new(Inspector::new(), None);
    let queue = fixture.handle.output.clone();
    let mut stream = TerminalOutput(queue.clone());
    assert!(queue.push(vec![b'x'; 65536]));
    let producing = queue.clone();
    let writer = tokio::task::spawn_blocking(move || producing.push(vec![b'y'; 10]));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !queue.waiting.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut bytes = vec![0; 65536];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, vec![b'x'; 65536]);
    assert!(writer.await.unwrap());
    let mut bytes = [0; 10];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, [b'y'; 10]);
    queue.push(vec![b'z'; 65536]);
    let producing = queue.clone();
    let writer = tokio::task::spawn_blocking(move || producing.push(vec![b'w']));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !queue.waiting.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.handle.terminate_shared().await.unwrap();
    assert!(writer.await.unwrap());
}
#[tokio::test]
async fn runs_owner_cleanup_once_after_repeated_failed_managed_termination_attempts() {
    let first = SharedError::new(error("first"));
    let second = SharedError::new(error("second"));
    let owner = Owner::new(
        None,
        vec![
            Err(first.clone()),
            Err(second.clone()),
            Err(first),
            Err(second),
        ],
    );
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    assert!(fixture.handle.terminate_shared().await.is_err());
    assert!(fixture.handle.terminate_shared().await.is_err());
    assert_eq!(owner.cleanups.load(Ordering::Acquire), 0);
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    assert_eq!(owner.cleanups.load(Ordering::Acquire), 1);
    fixture.handle.cleanup_managed_owner();
    assert_eq!(owner.cleanups.load(Ordering::Acquire), 1);
}
#[tokio::test]
async fn waits_for_the_pty_exit_after_the_managed_range_becomes_empty() {
    let owner = Owner::new(None, vec![Ok(())]);
    let fixture = Fixture::new(Inspector::new(), Some(owner.clone()));
    owner.connect(&fixture);
    let future = fixture.handle.terminate_shared();
    assert!(future.clone().now_or_never().is_none());
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    future.await.unwrap();
    assert_eq!(*lock(&owner.signals), [rustix::process::Signal::TERM]);
    assert_eq!(owner.cleanups.load(Ordering::Acquire), 1);
}
#[tokio::test]
async fn keeps_an_early_exit_wait_pending_through_descendant_cleanup() {
    let inspector = Inspector::new();
    {
        let mut world = lock(&inspector.0);
        world.rows.push(row(124, 123, 123, "child"));
        world.remove_on_signal = false;
    }
    let fixture = Fixture::new(inspector.clone(), None);
    fixture.emit_exit(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    });
    let future = fixture.handle.terminate_shared();
    let waiting = tokio::spawn(future);
    tokio::time::timeout(Duration::from_secs(1), async {
        while lock(&inspector.0).kills.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!waiting.is_finished());
    assert!(!lock(&fixture.handle.state).quiescent);
    lock(&inspector.0).rows.retain(|row| row.pid != 124);
    waiting.await.unwrap().unwrap();
    assert!(lock(&fixture.handle.state).quiescent);
}
#[tokio::test]
async fn retains_root_exited_native_work_with_failed_observation_remaining_unknown() {
    for fails in [false, true] {
        let owner = Owner::new(None, vec![]);
        let inspector = Inspector::new();
        let fixture = Fixture::new(inspector.clone(), Some(owner));
        fixture.emit_exit(SubprocessOutcome {
            exit_code: Some(0),
            signal: None,
        });
        lock(&inspector.0).rows.clear();
        lock(&inspector.0).read_error = fails;
        assert_eq!(
            fixture.control().inspect_activity().await.unwrap().state,
            SubprocessTerminalActivityState::Unknown
        );
        lock(&fixture.handle.state).range_empty = !fails;
        assert_eq!(
            fixture.control().inspect_activity().await.unwrap().state,
            if fails {
                SubprocessTerminalActivityState::Unknown
            } else {
                SubprocessTerminalActivityState::Idle
            }
        );
    }
}
