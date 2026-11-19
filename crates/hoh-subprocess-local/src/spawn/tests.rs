use super::*;
#[derive(Default)]
struct World {
    probe: Option<rustix::io::Errno>,
    probes: usize,
    members: Vec<Option<Option<u32>>>,
    known: Vec<Option<u32>>,
    signals: Vec<rustix::process::Signal>,
}
#[derive(Default)]
struct Operations(Mutex<World>);
impl FallbackOperations for Operations {
    fn probe_group(&self, _: rustix::process::Pid) -> Result<(), rustix::io::Errno> {
        let mut state = lock(&self.0);
        state.probes += 1;
        state.probe.map_or(Ok(()), Err)
    }
    fn signal_group(
        &self,
        _: rustix::process::Pid,
        signal: rustix::process::Signal,
    ) -> Result<(), rustix::io::Errno> {
        lock(&self.0).signals.push(signal);
        Ok(())
    }
    fn signal_direct(&self, _: u32, _: rustix::process::Signal) {
        panic!("settled direct PID must not be signalled");
    }
    fn live_member(&self, _: u32, known: Option<u32>) -> Option<Option<u32>> {
        let mut state = lock(&self.0);
        state.known.push(known);
        state.members.remove(0)
    }
}
#[tokio::test]
async fn does_not_wait_for_a_linux_group_that_has_only_zombie_members() {
    let operations = Arc::new(Operations::default());
    lock(&operations.0).members = vec![Some(None)];
    let owner = FallbackOwner::with_operations(
        Some(4242),
        Arc::new(AtomicBool::new(true)),
        operations.clone(),
    );
    owner.wait_for_exit().await.unwrap();
    owner.signal(rustix::process::Signal::TERM);
    owner.terminate_for_host_exit();
    let state = lock(&operations.0);
    assert_eq!(state.probes, 1);
    assert!(state.signals.is_empty());
}
#[tokio::test(start_paused = true)]
async fn passes_the_last_found_linux_group_member_back_to_the_next_probe() {
    let operations = Arc::new(Operations::default());
    lock(&operations.0).members = vec![Some(Some(4242)), None, Some(None)];
    let owner = FallbackOwner::with_operations(
        Some(4242),
        Arc::new(AtomicBool::new(true)),
        operations.clone(),
    );
    owner.wait_for_exit().await.unwrap();
    assert_eq!(*lock(&operations.0).known, [None, Some(4242), Some(4242)]);
}
#[test]
fn treats_a_vanished_group_probe_as_quiescent_without_signalling() {
    let operations = Arc::new(Operations::default());
    lock(&operations.0).probe = Some(rustix::io::Errno::SRCH);
    let owner = FallbackOwner::with_operations(
        Some(4242),
        Arc::new(AtomicBool::new(true)),
        operations.clone(),
    );
    owner.signal(rustix::process::Signal::TERM);
    owner.signal(rustix::process::Signal::KILL);
    owner.terminate_for_host_exit();
    let state = lock(&operations.0);
    assert_eq!(state.probes, 1);
    assert!(state.signals.is_empty());
}
#[tokio::test]
async fn treats_an_eperm_group_probe_as_still_alive() {
    let operations = Arc::new(Operations::default());
    lock(&operations.0).probe = Some(rustix::io::Errno::PERM);
    let owner = FallbackOwner::with_operations(
        Some(4242),
        Arc::new(AtomicBool::new(true)),
        operations.clone(),
    );
    assert!(owner.alive());
    owner.signal(rustix::process::Signal::TERM);
    assert_eq!(
        *lock(&operations.0).signals,
        [rustix::process::Signal::TERM]
    );
    assert!(lock(&operations.0).known.is_empty());
}

#[tokio::test]
async fn cancels_escalation_when_the_group_vanishes_after_term_before_pipes_drain() {
    let operations = Arc::new(Operations::default());
    let direct_settled = Arc::new(AtomicBool::new(false));
    let owner =
        FallbackOwner::with_operations(Some(4242), direct_settled.clone(), operations.clone());

    owner.signal(rustix::process::Signal::TERM);
    assert_eq!(
        *lock(&operations.0).signals,
        [rustix::process::Signal::TERM]
    );

    direct_settled.store(true, Ordering::Release);
    lock(&operations.0).probe = Some(rustix::io::Errno::SRCH);
    owner.wait_for_exit().await.unwrap();
    owner.signal(rustix::process::Signal::KILL);

    assert_eq!(
        *lock(&operations.0).signals,
        [rustix::process::Signal::TERM],
        "a vanished group must not receive the delayed SIGKILL"
    );
}
