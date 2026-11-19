use super::*;
use crate::{
    managed_owner::{BoundProcessOwner, RangeExit},
    output::prepare_managed_process_binding,
    spawn::{ManagedProcessLaunch, bind_managed_process},
};
use futures_util::FutureExt;
use std::sync::atomic::AtomicUsize;

struct Owner {
    failure: Option<SharedError>,
    stop: AtomicUsize,
    force: AtomicUsize,
    panic_on_force: bool,
}
impl BoundProcessOwner for Owner {
    fn signal(&self, _: rustix::process::Signal) {
        self.stop.fetch_add(1, Ordering::AcqRel);
    }
    fn wait_for_exit(&self) -> RangeExit {
        futures_util::future::ready(match &self.failure {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        })
        .boxed()
        .shared()
    }
    fn terminate_for_host_exit(&self) {
        self.force.fetch_add(1, Ordering::AcqRel);
        assert!(!self.panic_on_force, "ordinary failed");
    }
}
fn bind(ctx: &Context, managed: &Arc<Managed>, owner: Arc<Owner>) {
    let request = SubprocessSpawnSpec {
        argv: vec!["unused".into()],
        cwd: "/tmp".into(),
        env: None,
        grace_ms: 10.0,
        signal: None,
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Inherit,
            stderr: SubprocessOutputMode::Inherit,
            control: None,
        },
    };
    let handle = bind_managed_process(
        ctx,
        request,
        ManagedProcessLaunch {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            direct: subprocess_done(futures_util::future::pending()),
            owner,
        },
        prepare_managed_process_binding(&managed.storage, None, Arc::new(|_, _| Ok(()))).unwrap(),
    )
    .unwrap();
    lock(&managed.processes).push(handle.lifecycle);
}
#[tokio::test]
async fn observes_range_failure_without_waiting_for_a_stuck_direct_result() {
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let managed = Arc::new(Managed::default());
    let failure = SharedError::new(LocalError::Error("managed range became unreadable".into()));
    let owner = Arc::new(Owner {
        failure: Some(failure.clone()),
        stop: AtomicUsize::new(0),
        force: AtomicUsize::new(0),
        panic_on_force: false,
    });
    bind(&ctx, &managed, owner.clone());
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), managed.clone().dispose())
        .await
        .unwrap()
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
    assert_eq!(owner.force.load(Ordering::Acquire), 1);
    assert_eq!(lock(&managed.processes).len(), 1);
    // Direct deliberately never settles; drop the test kernel rather than waiting for that fixture.
    drop(kernel);
}
#[tokio::test]
async fn contains_each_host_exit_termination_failure_and_continues_with_the_other_targets() {
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let managed = Arc::new(Managed::default());
    let owners = [true, false].map(|panic_on_force| {
        Arc::new(Owner {
            failure: None,
            stop: AtomicUsize::new(0),
            force: AtomicUsize::new(0),
            panic_on_force,
        })
    });
    for owner in &owners {
        bind(&ctx, &managed, owner.clone());
    }
    managed.force_stop();
    assert_eq!(owners[0].force.load(Ordering::Acquire), 1);
    assert_eq!(owners[1].force.load(Ordering::Acquire), 1);
    drop(kernel);
}
#[tokio::test]
async fn waits_for_every_terminal_cleanup_and_aggregates_teardown_failures() {
    let managed = Arc::new(Managed::default());
    let terminals = [true, false, true].map(crate::terminal::tests::retained_terminal);
    lock(&managed.terminals).extend(terminals.iter().cloned());
    let error = managed.dispose().await.unwrap_err();
    let LocalError::Teardown(errors) = error.downcast_ref::<LocalError>().unwrap() else {
        panic!("expected aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert!(
        errors
            .iter()
            .all(|error| error.to_string() == "terminal cleanup failed; surviving pid: 123")
    );
    assert!(!terminals[1].running());
}
#[tokio::test]
async fn reports_one_cleanup_failure_without_wrapping_it() {
    let managed = Arc::new(Managed::default());
    let terminal = crate::terminal::tests::retained_terminal(true);
    let original = terminal.terminate_shared();
    lock(&managed.terminals).push(terminal);
    let error = managed.dispose().await.unwrap_err();
    let error = SharedError::of(error.as_ref()).unwrap();
    assert!(error.ptr_eq(&original.await.unwrap_err()));
    assert!(
        matches!(error.downcast_ref::<LocalError>(), Some(LocalError::Error(message)) if message == "terminal cleanup failed; surviving pid: 123")
    );
}
