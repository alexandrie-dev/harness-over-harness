use hoh_subprocess_local::linux_scope_selector::*;
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct World {
    probe: Option<Vec<String>>,
    listener: Option<String>,
    confirm: bool,
    launcher: Option<String>,
    during: Option<Box<World>>,
    calls: [usize; 4],
}
impl World {
    fn healthy() -> Self {
        Self {
            probe: Some(vec!["--expand-environment=no".into()]),
            listener: Some("manager-1".into()),
            confirm: true,
            launcher: Some("systemd-run-255".into()),
            during: None,
            calls: [0; 4],
        }
    }
    fn absent() -> Self {
        Self {
            probe: None,
            listener: None,
            confirm: false,
            launcher: Some("systemd-run-245".into()),
            during: None,
            calls: [0; 4],
        }
    }
}
#[derive(Clone)]
struct Observe(Arc<Mutex<World>>);
impl LinuxScopeObservations for Observe {
    fn probe(&self) -> Result<Option<Vec<String>>, hoh_error::BoxError> {
        let mut state = self.0.lock().unwrap();
        state.calls[0] += 1;
        let value = state.probe.clone();
        if let Some(next) = state.during.take() {
            let calls = state.calls;
            *state = *next;
            state.calls = calls;
        }
        Ok(value)
    }
    fn manager_listener(&self) -> Option<String> {
        let mut state = self.0.lock().unwrap();
        state.calls[1] += 1;
        state.listener.clone()
    }
    fn confirm_manager(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        state.calls[2] += 1;
        state.confirm
    }
    fn launcher_identity(&self) -> Option<String> {
        let mut state = self.0.lock().unwrap();
        state.calls[3] += 1;
        state.launcher.clone()
    }
}
enum Step {
    Launch,
    Set(World),
    During(World),
    NotStarted(u64),
}
fn summary(decision: LinuxScopeDecision) -> String {
    let (mut result, probe) = match decision {
        LinuxScopeDecision::Scope {
            reason,
            literal_argv,
            generation,
            probe,
        } => (
            format!("scope[{}]#{generation} {reason}", literal_argv.join(" ")),
            probe,
        ),
        LinuxScopeDecision::Fallback { reason, probe } => (format!("fallback {reason}"), probe),
    };
    if let Some(probe) = probe {
        result.push_str(&format!(
            " after {}",
            match probe {
                LinuxProbeTrigger::First => "first",
                LinuxProbeTrigger::ScopeNotStarted => "scope-not-started",
                LinuxProbeTrigger::NewListener => "new-listener",
                LinuxProbeTrigger::LauncherChanged => "launcher-changed",
            }
        ));
    }
    result
}
fn run(initial: World, steps: Vec<Step>, expected: &[&str], probes: usize) {
    let observe = Observe(Arc::new(Mutex::new(initial)));
    let mut selector = LinuxScopeSelector::new(observe.clone());
    let mut decisions = vec![];
    for step in steps {
        match step {
            Step::Launch => decisions.push(summary(selector.select().unwrap())),
            Step::Set(world) => {
                let mut state = observe.0.lock().unwrap();
                let calls = state.calls;
                *state = world;
                state.calls = calls;
            }
            Step::During(world) => observe.0.lock().unwrap().during = Some(Box::new(world)),
            Step::NotStarted(generation) => selector.scope_not_started(generation),
        }
    }
    assert_eq!(decisions, expected);
    assert_eq!(observe.0.lock().unwrap().calls[0], probes);
}
use Step::*;
const S: &str = "scope[--expand-environment=no]#1 probe-available after first";
const C: &str = "scope[--expand-environment=no]#1 manager-confirmed";
#[test]
fn no_user_manager_probe_once_then_fall_back_without_probing() {
    run(
        World::absent(),
        vec![Launch, Launch, Launch],
        &[
            "fallback probe-unavailable after first",
            "fallback no-listener",
            "fallback no-listener",
        ],
        1,
    );
}
#[test]
fn systemd_254_and_later_probe_once_then_confirm_the_manager_per_launch() {
    run(
        World::healthy(),
        vec![Launch, Launch, Launch],
        &[S, C, C],
        1,
    );
}
#[test]
fn systemd_236_to_253_scope_launches_carry_no_expansion_option() {
    let mut old = World::healthy();
    old.probe = Some(vec![]);
    old.launcher = Some("systemd-run-249".into());
    run(
        old,
        vec![Launch, Launch],
        &[
            "scope[]#1 probe-available after first",
            "scope[]#1 manager-confirmed",
        ],
        1,
    );
}
#[test]
fn a_manager_that_stops_and_returns_under_a_positive_verdict_falls_back_only_while_it_is_gone() {
    let mut gone = World::healthy();
    gone.listener = None;
    gone.confirm = false;
    let mut returned = World::healthy();
    returned.listener = Some("manager-2".into());
    run(
        World::healthy(),
        vec![Launch, Set(gone), Launch, Launch, Set(returned), Launch],
        &[
            S,
            "fallback manager-unconfirmed",
            "fallback manager-unconfirmed",
            C,
        ],
        1,
    );
}
#[test]
fn a_manager_hung_at_the_first_probe_is_not_probed_again_until_a_new_manager_listens() {
    let mut hung = World::absent();
    hung.listener = Some("hung-manager".into());
    let mut fresh = World::healthy();
    fresh.listener = Some("manager-2".into());
    run(
        hung,
        vec![Launch, Launch, Launch, Set(fresh), Launch, Launch],
        &[
            "fallback probe-unavailable after first",
            "fallback same-listener",
            "fallback same-listener",
            "scope[--expand-environment=no]#2 probe-available after new-listener",
            "scope[--expand-environment=no]#2 manager-confirmed",
        ],
        2,
    );
}
#[test]
fn a_manager_that_hangs_under_a_positive_verdict_costs_one_probe_after_its_scope_never_starts() {
    let mut initial = World::healthy();
    initial.listener = Some("hung-manager".into());
    let mut hung = initial.clone();
    hung.probe = None;
    let mut fresh = World::healthy();
    fresh.listener = Some("manager-2".into());
    run(
        initial,
        vec![
            Launch,
            Launch,
            Set(hung),
            Launch,
            NotStarted(1),
            Launch,
            NotStarted(1),
            Launch,
            Set(fresh),
            Launch,
        ],
        &[
            S,
            C,
            C,
            "fallback probe-unavailable after scope-not-started",
            "fallback same-listener",
            "scope[--expand-environment=no]#3 probe-available after new-listener",
        ],
        3,
    );
}
#[test]
fn a_scope_that_never_starts_under_a_healthy_manager_probes_again_and_keeps_the_scope() {
    run(
        World::healthy(),
        vec![Launch, NotStarted(1), Launch, Launch],
        &[
            S,
            "scope[--expand-environment=no]#2 probe-available after scope-not-started",
            "scope[--expand-environment=no]#2 manager-confirmed",
        ],
        2,
    );
}
#[test]
fn a_manager_that_starts_after_a_failed_first_probe_is_used_without_a_restart() {
    run(
        World::absent(),
        vec![Launch, Launch, Set(World::healthy()), Launch, Launch],
        &[
            "fallback probe-unavailable after first",
            "fallback no-listener",
            "scope[--expand-environment=no]#2 probe-available after new-listener",
            "scope[--expand-environment=no]#2 manager-confirmed",
        ],
        2,
    );
}
#[test]
fn a_manager_that_starts_during_a_failed_probe_counts_as_a_new_listener() {
    run(
        World::absent(),
        vec![During(World::healthy()), Launch, Launch],
        &[
            "fallback probe-unavailable after first",
            "scope[--expand-environment=no]#2 probe-available after new-listener",
        ],
        2,
    );
}
#[test]
fn a_systemd_run_upgrade_across_v254_probes_the_literal_argv_options_again() {
    let mut old = World::healthy();
    old.probe = Some(vec![]);
    old.launcher = Some("systemd-run-253".into());
    let mut new = World::healthy();
    new.launcher = Some("systemd-run-258".into());
    run(
        old,
        vec![Launch, Launch, Set(new), Launch, Launch],
        &[
            "scope[]#1 probe-available after first",
            "scope[]#1 manager-confirmed",
            "scope[--expand-environment=no]#2 probe-available after launcher-changed",
            "scope[--expand-environment=no]#2 manager-confirmed",
        ],
        2,
    );
}
#[test]
fn a_removed_systemd_run_turns_a_positive_verdict_negative() {
    let mut gone = World::healthy();
    gone.probe = None;
    gone.launcher = None;
    run(
        World::healthy(),
        vec![Launch, Set(gone), Launch, Launch],
        &[
            S,
            "fallback probe-unavailable after launcher-changed",
            "fallback same-listener",
        ],
        2,
    );
}
#[test]
fn consults_only_the_observations_each_verdict_needs() {
    let mut world = World::absent();
    world.launcher = Some("systemd-run".into());
    world.confirm = true;
    let observe = Observe(Arc::new(Mutex::new(world)));
    let mut selector = LinuxScopeSelector::new(observe.clone());
    selector.select().unwrap();
    selector.select().unwrap();
    assert_eq!(observe.0.lock().unwrap().calls, [1, 2, 0, 1]);
    {
        let mut world = observe.0.lock().unwrap();
        world.probe = Some(vec![]);
        world.listener = Some("manager-1".into());
    }
    selector.select().unwrap();
    observe.0.lock().unwrap().calls[1] = 0;
    selector.select().unwrap();
    let calls = observe.0.lock().unwrap().calls;
    assert_eq!(calls[1], 0);
    assert_eq!(calls[2], 1);
}
