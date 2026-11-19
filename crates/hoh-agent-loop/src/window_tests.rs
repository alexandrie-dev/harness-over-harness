//! Deterministic checks of the driver's racing windows: a competing call is injected at the exact
//! point between two steps of a transition (`Window`), instead of hoping a stress run hits it.
#[path = "../tests/common/mod.rs"]
mod common;

use crate::agent::{Window, set_window_probe};
use common::*;
use hoh_agent::{Agent, CancelOptions};
use hoh_session::AgentCancelCause;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Run `action` the first time `window` is reached for the agent `id`.
fn once_at(id: &str, window: Window, action: impl Fn() + Send + Sync + 'static) {
    let fired = AtomicBool::new(false);
    set_window_probe(
        id,
        Arc::new(move |reached| {
            if reached == window && !fired.swap(true, Ordering::SeqCst) {
                action();
            }
        }),
    );
}

fn user_cancel(agent: &Arc<dyn Agent>) {
    agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect("cancel");
}

async fn idle_within(agent: &Arc<dyn Agent>) {
    tokio::time::timeout(std::time::Duration::from_secs(5), agent.when_idle())
        .await
        .expect("the agent settles idle");
}

#[tokio::test]
async fn a_followup_between_the_clear_and_the_abort_of_a_cancel_is_replayed() {
    let adapter = MockAdapter::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("late reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-cancel-followup").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    {
        let agent = agent.clone();
        once_at("win-cancel-followup", Window::CancelAfterClear, move || {
            agent.followup(user_message("late")).expect("followup");
        });
    }

    user_cancel(&agent);
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["active", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

#[tokio::test]
async fn a_steer_between_the_clear_and_the_abort_of_a_cancel_is_replayed_as_the_next_turn() {
    let adapter = MockAdapter::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("late reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-cancel-steer").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    {
        let agent = agent.clone();
        once_at("win-cancel-steer", Window::CancelAfterClear, move || {
            agent.steer(user_message("late")).expect("steer");
        });
    }

    user_cancel(&agent);
    idle_within(&agent).await;

    assert_eq!(adapter.request_count(), 2);
    assert!(user_texts(&agent).contains(&"late".to_owned()));
}

#[tokio::test]
async fn an_inject_between_the_clear_and_the_abort_of_a_cancel_wakes_nothing() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("unused"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-cancel-inject").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    {
        let agent = agent.clone();
        once_at("win-cancel-inject", Window::CancelAfterClear, move || {
            agent.inject(user_message("note")).expect("inject");
        });
    }

    user_cancel(&agent);
    idle_within(&agent).await;

    // The source's inject never wakes: it stays queued for the next wake.
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(agent.inbox().next_step().len(), 1);
}

#[tokio::test]
async fn a_queued_message_parked_by_an_aborted_turn_is_not_replayed_at_settle() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("unused"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-parked").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    send_text(&agent, "parked");

    agent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })
        .expect("cancel");
    idle_within(&agent).await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(agent.inbox().next_turn().len(), 1);
}

/// A second wake landing right after the first one reserved the running phase (before its driver
/// exists) finds the agent running and latches: one driver, never a second concurrent one. The
/// check-and-reserve itself is one critical section, so no window exists between them.
#[tokio::test]
async fn a_wake_right_after_the_reservation_of_the_running_phase_starts_no_second_driver() {
    let adapter = MockAdapter::chunks(vec![text_response("one"), text_response("two")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-two-drivers").await;
    {
        let agent = agent.clone();
        once_at("win-two-drivers", Window::AfterReserve, move || {
            agent.followup(user_message("second")).expect("followup");
        });
    }

    send_text(&agent, "first");
    idle_within(&agent).await;

    let mut open = false;
    for kind in event_types(&agent) {
        match kind.as_str() {
            "turn/start" => {
                assert!(!open, "a second turn opened inside an open turn");
                open = true;
            }
            "turn/end" => open = false,
            _ => {}
        }
    }
    assert_eq!(user_texts(&agent), ["first", "second"]);
    assert_eq!(adapter.request_count(), 2);
}

/// A boom in the steps (the failure path of `turn_steps`): the window right after its outcome,
/// before the driver forms the turn's ending, is part of the same step as the outcome.
#[tokio::test]
async fn a_followup_right_after_the_steps_failed_is_replayed() {
    let adapter = MockAdapter::new(vec![
        Entry::Fail("boom"),
        Entry::Chunks(text_response("ok")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-steps-failed").await;
    {
        let agent = agent.clone();
        once_at(
            "win-steps-failed",
            Window::AfterTurnStepsOutcome,
            move || {
                agent.followup(user_message("late")).expect("followup");
            },
        );
    }

    send_text(&agent, "first");
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["first", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

/// Work staged before a rejected step stays parked: the wake that staged it was counted before
/// the driver's last look at the queue, so it earns no extra lap.
#[tokio::test]
async fn work_left_in_the_queue_by_a_rejected_step_is_not_replayed() {
    let adapter = MockAdapter::chunks(vec![text_response("unused")]);
    let env = harness(adapter.clone()).await;
    env.ctx
        .on_waterfall::<hoh_agent::AgentPreStep, hoh_agent::PreStepDecision, _>(
            move |event, next| async move {
                if format!("{:?}", event.messages).contains("blocked") {
                    // Staged after the claim, like the source's interception cases.
                    event.agent.steer(user_message("staged steering"))?;
                    return Ok(hoh_agent::PreStepDecision::Reject);
                }
                next.run().await
            },
        )
        .expect("rejecting listener");
    let agent = create_agent(&env, "win-rejected-parked").await;

    send_text(&agent, "blocked prompt");
    idle_within(&agent).await;

    assert_eq!(adapter.request_count(), 0);
    assert_eq!(agent.inbox().next_step().len(), 1);
}

/// What a scenario for one `Window` looks like. The `match` below is exhaustive on purpose: a new
/// window cannot be added without deciding how it is exercised.
#[derive(Clone, Copy)]
enum Reach {
    /// Reached by any driven turn (successful or failing).
    Turn,
    /// Reached by cancelling a running turn.
    Cancel,
    /// Reached inside the wake critical section: only a blocked competitor can act there.
    WakeSection,
}

fn reach(window: Window) -> Reach {
    match window {
        Window::CancelAfterClear => Reach::Cancel,
        Window::WakeCheckedIdle => Reach::WakeSection,
        Window::AfterReserve
        | Window::SettleLatchRead
        | Window::AfterTurnStepsOutcome
        | Window::BeforeSettleIdle
        | Window::AfterTurnOutcome => Reach::Turn,
    }
}

const ALL_WINDOWS: [Window; 6 + 1] = [
    Window::CancelAfterClear,
    Window::AfterReserve,
    Window::WakeCheckedIdle,
    Window::SettleLatchRead,
    Window::AfterTurnStepsOutcome,
    Window::BeforeSettleIdle,
    Window::AfterTurnOutcome,
];

/// The sieve: a followup landing at EVERY probe point, once per point, on a succeeding and on a
/// failing turn, always earns its own request and leaves the inbox empty.
#[tokio::test]
async fn a_followup_at_any_window_of_the_driver_is_never_lost() {
    for (index, window) in ALL_WINDOWS.into_iter().enumerate() {
        match reach(window) {
            Reach::Turn => {
                for failing in [false, true] {
                    let first = if failing {
                        Entry::Fail("boom")
                    } else {
                        Entry::Chunks(text_response("first"))
                    };
                    let adapter =
                        MockAdapter::new(vec![first, Entry::Chunks(text_response("late reply"))]);
                    let env = harness(adapter.clone()).await;
                    let id = format!("sieve-{index}-{failing}");
                    let agent = create_agent(&env, &id).await;
                    let slot = Competitor::default();
                    let blocked = Arc::new(AtomicBool::new(false));
                    if window == Window::SettleLatchRead {
                        // The settle section holds the lock: only a competitor can act inside.
                        once_at(
                            &id,
                            window,
                            compete_while_held(&agent, &blocked, &slot, competing_late),
                        );
                    } else {
                        let agent = agent.clone();
                        once_at(&id, window, move || competing_late(&agent));
                    }

                    send_text(&agent, "first");
                    until(|| window != Window::SettleLatchRead || slot.lock().unwrap().is_some())
                        .await;
                    if window == Window::SettleLatchRead {
                        join_competitor(&slot);
                    }
                    idle_within(&agent).await;

                    let what = format!("{window:?}, failing={failing}");
                    if window == Window::AfterReserve && failing {
                        // Queued before the driver looked at anything: like the source, the
                        // failing first turn leaves it parked (no wake came after its last look).
                        assert_eq!(user_texts(&agent), ["first"], "{what}");
                        assert_eq!(adapter.request_count(), 1, "{what}");
                        assert_eq!(agent.inbox().next_turn().len(), 1, "{what}");
                    } else {
                        assert_eq!(user_texts(&agent), ["first", "late"], "{what}");
                        assert_eq!(adapter.request_count(), 2, "{what}");
                        assert!(agent.inbox().next_turn().is_empty(), "{what}");
                    }
                    env.close().await;
                }
            }
            Reach::Cancel => {
                let adapter = MockAdapter::new(vec![
                    Entry::Hang,
                    Entry::Chunks(text_response("late reply")),
                ]);
                let env = harness(adapter.clone()).await;
                let id = format!("sieve-{index}");
                let agent = create_agent(&env, &id).await;
                send_text(&agent, "first");
                until(|| adapter.request_count() == 1).await;
                {
                    let agent = agent.clone();
                    once_at(&id, window, move || competing_late(&agent));
                }
                user_cancel(&agent);
                idle_within(&agent).await;
                assert_eq!(user_texts(&agent), ["first", "late"], "{window:?}");
                assert_eq!(adapter.request_count(), 2, "{window:?}");
                assert!(agent.inbox().next_turn().is_empty(), "{window:?}");
                env.close().await;
            }
            Reach::WakeSection => {
                // Inside the idle check of `wake_driver`: a competing wake stays blocked until the
                // reservation is made, then finds the agent running (its own test above).
                let adapter = MockAdapter::chunks(vec![text_response("first")]);
                let env = harness(adapter.clone()).await;
                let id = format!("sieve-{index}");
                let agent = create_agent(&env, &id).await;
                let (blocked, slot) = (Arc::new(AtomicBool::new(false)), Competitor::default());
                once_at(
                    &id,
                    window,
                    compete_while_held(&agent, &blocked, &slot, competing_maintenance),
                );
                send_text(&agent, "first");
                join_competitor(&slot);
                idle_within(&agent).await;
                assert!(blocked.load(Ordering::SeqCst), "{window:?}");
                assert_eq!(adapter.request_count(), 1, "{window:?}");
                env.close().await;
            }
        }
    }
}

fn competing_late(agent: &Arc<dyn Agent>) {
    agent.followup(user_message("late")).expect("followup");
}

type Competitor = Arc<Mutex<Option<std::thread::JoinHandle<()>>>>;

/// From INSIDE a critical section of the driver (the state lock is held by the calling thread):
/// start a competing waking send on another thread and report whether it stayed blocked. A
/// section that were split in two (check, then write) would let it through in the gap.
fn compete_while_held(
    agent: &Arc<dyn Agent>,
    blocked: &Arc<AtomicBool>,
    slot: &Competitor,
    action: fn(&Arc<dyn Agent>),
) -> impl Fn() + Send + Sync + 'static {
    let (agent, blocked, slot) = (agent.clone(), blocked.clone(), slot.clone());
    move || {
        let handle = tokio::runtime::Handle::current();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let agent = agent.clone();
        let competitor = std::thread::spawn(move || {
            let _runtime = handle.enter();
            action(&agent);
            let _ = done_tx.send(());
        });
        blocked.store(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(150))
                .is_err(),
            Ordering::SeqCst,
        );
        *slot.lock().unwrap() = Some(competitor);
    }
}

/// A competing waking send (its own session append is serialized by the test, never concurrent
/// with the driver's: the session itself rejects concurrent appends).
fn competing_followup(agent: &Arc<dyn Agent>) {
    agent
        .followup(user_message("second"))
        .expect("competing followup");
}

/// A competing admission of maintenance work: it takes the same idle check and writes no event.
fn competing_maintenance(agent: &Arc<dyn Agent>) {
    let refused = hoh_agent::run_maintenance(&**agent, |_signal: hoh_timeout::AbortSignal| async {
        Ok(())
    })
    .err()
    .expect("the agent is running, so maintenance is refused");
    assert!(
        refused.to_string().contains("already has active work"),
        "{refused}"
    );
}

fn join_competitor(slot: &Competitor) {
    let competitor = slot.lock().unwrap().take().expect("a competitor started");
    competitor.join().expect("competitor joined");
}

#[tokio::test]
async fn a_followup_after_the_driver_decided_to_end_but_before_it_settles_idle_is_replayed() {
    let adapter = MockAdapter::chunks(vec![text_response("first"), text_response("second")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-before-settle").await;
    {
        let agent = agent.clone();
        once_at("win-before-settle", Window::BeforeSettleIdle, move || {
            agent.followup(user_message("late")).expect("followup");
        });
    }

    send_text(&agent, "first");
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["first", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

#[tokio::test]
async fn work_parked_by_a_keep_inbox_abort_is_not_replayed_by_the_unseen_wake_count() {
    // Wakes counted before the driver's last look at the queue are accounted for: leftovers
    // parked by a `keep_inbox` abort are not replayed.
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-parked-closing").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    send_text(&agent, "parked");
    agent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })
        .expect("cancel");
    idle_within(&agent).await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(agent.inbox().next_turn().len(), 1);
}

/// The check of the idle phase and the reservation of the running phase are ONE critical section:
/// a competing wake started from inside it stays blocked until it ends, then finds the agent
/// running and cannot start a second driver.
#[tokio::test]
async fn a_competing_wake_cannot_enter_between_the_idle_check_and_the_reservation() {
    let adapter = MockAdapter::chunks(vec![text_response("one"), text_response("two")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-check-reserve").await;
    let (blocked, slot) = (Arc::new(AtomicBool::new(false)), Competitor::default());
    once_at(
        "win-check-reserve",
        Window::WakeCheckedIdle,
        compete_while_held(&agent, &blocked, &slot, competing_maintenance),
    );

    send_text(&agent, "first");
    join_competitor(&slot);
    idle_within(&agent).await;

    assert!(
        blocked.load(Ordering::SeqCst),
        "a competing wake got through between the idle check and the reservation"
    );
    let mut open = false;
    for kind in event_types(&agent) {
        match kind.as_str() {
            "turn/start" => {
                assert!(!open, "a second turn opened inside an open turn");
                open = true;
            }
            "turn/end" => open = false,
            _ => {}
        }
    }
    assert_eq!(user_texts(&agent), ["first"]);
    assert_eq!(adapter.request_count(), 1);
}

/// Reading the latch and leaving the running phase are ONE critical section: a competing wake
/// started from inside it stays blocked, then finds the agent idle and starts its own driver (the
/// old read-then-write split let a wake latch on the already-read value and be lost).
#[tokio::test]
async fn a_competing_wake_cannot_enter_between_the_latch_read_and_the_idle_transition() {
    let adapter = MockAdapter::chunks(vec![text_response("first"), text_response("second")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-latch-idle").await;
    let (blocked, slot) = (Arc::new(AtomicBool::new(false)), Competitor::default());
    once_at(
        "win-latch-idle",
        Window::SettleLatchRead,
        compete_while_held(&agent, &blocked, &slot, competing_followup),
    );

    send_text(&agent, "first");
    // The settle runs on the driver task: wait for the competitor to exist, then for all work.
    until(|| slot.lock().unwrap().is_some()).await;
    join_competitor(&slot);
    idle_within(&agent).await;

    assert!(
        blocked.load(Ordering::SeqCst),
        "a competing wake got through between the latch read and the idle transition"
    );
    assert_eq!(user_texts(&agent), ["first", "second"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

/// A failed turn: the error path fixes its outcome inside `turn_once`, marking the driver closing
/// with it, so a wake landing right after the outcome is replayed (the source's rejected `turn()`
/// and the `finally` of `kick` are one microtask).
#[tokio::test]
async fn a_followup_right_after_a_failed_turn_outcome_is_replayed() {
    let adapter = MockAdapter::new(vec![
        Entry::Fail("boom"),
        Entry::Chunks(text_response("ok")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-error-outcome").await;
    {
        let agent = agent.clone();
        once_at("win-error-outcome", Window::AfterTurnOutcome, move || {
            agent.followup(user_message("late")).expect("followup");
        });
    }

    send_text(&agent, "first");
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["first", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

/// The same for the other exits of `turn`: a turn that cannot record its ending, an aborted one
/// and one that fails at its start.
#[tokio::test]
async fn a_followup_right_after_a_turn_that_failed_to_record_its_end_is_replayed() {
    let adapter = MockAdapter::chunks(vec![text_response("one"), text_response("two")]);
    let env = harness(adapter.clone()).await;
    let _trap = env
        .ctx
        .get::<hoh_session::Sessions>()
        .expect("sessions")
        .register_message_projection(&env.ctx, Arc::new(TurnEndTrap))
        .expect("trap registers");
    let agent = create_agent(&env, "win-turn-end-failure").await;
    {
        let agent = agent.clone();
        once_at(
            "win-turn-end-failure",
            Window::AfterTurnOutcome,
            move || {
                agent.followup(user_message("late")).expect("followup");
            },
        );
    }

    send_text(&agent, "first");
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["first", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

#[tokio::test]
async fn a_followup_right_after_an_aborted_turn_outcome_is_replayed() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("after"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "win-abort-outcome").await;
    send_text(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    {
        let agent = agent.clone();
        once_at("win-abort-outcome", Window::AfterTurnOutcome, move || {
            agent.followup(user_message("late")).expect("followup");
        });
    }

    user_cancel(&agent);
    idle_within(&agent).await;

    assert_eq!(user_texts(&agent), ["active", "late"]);
    assert_eq!(adapter.request_count(), 2);
    assert!(agent.inbox().next_turn().is_empty());
}

/// Rejects the `turn/end` event at planning: the append that closes a turn fails.
struct TurnEndTrap;

impl hoh_session::SessionMessageProjection for TurnEndTrap {
    fn event_type(&self) -> &str {
        "turn/end"
    }
    fn project(
        &self,
        _event: &hoh_session::SessionEvent,
        _context: hoh_session::SessionMessageProjectionContext<'_>,
    ) -> hoh_session::Result<indexmap::IndexMap<hoh_session::SessionSeq, Arc<serde_json::Value>>>
    {
        Err("turn/end rejected by the trap".into())
    }
}

fn send_text(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).expect("followup");
}
