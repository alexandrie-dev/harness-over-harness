//! Races the source's single thread rules out and a multi-threaded host does not. Each test pins
//! the interleaving with a seam (a stalled cancel, a blocking listener, a barrier in the starter)
//! instead of hoping a loop hits it.

mod common;

use common::*;
use hoh_jobs::{JobEvent, JobHooks, JobRegistryApi, JobSettleCause, JobSpec, JobStatus};
use hoh_timeout::AbortController;
use serde_json::json;
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Duration,
};

/// A producer that settles inside its own `cancel` on another worker's time: `kill` must still end
/// `killed`, with cause `kill` and the reason in the detail, and never go back to `stopping`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_survives_a_producer_that_settles_inside_cancel() {
    let world = harness(json!({})).await;
    let events = collect(&world, all_owners(), None);
    for round in 0..10 {
        let mut p = producer_with(ProducerOptions {
            settle_on_cancel: Some(killed()),
            cancel_stall: Some(Duration::from_millis(40)),
            ..ProducerOptions::default()
        });
        let id = start_ok(&world, p.spec());
        world.jobs().kill(&id, None, Some("by hand")).unwrap();
        tick().await;
        let view = world.jobs().get(&id, None).unwrap();
        assert_eq!(view.status, JobStatus::Killed, "round {round}");
        assert_eq!(view.detail.as_deref(), Some("by hand"), "round {round}");
        let settled: Vec<_> = lock(&events)
            .iter()
            .filter_map(|event| match event {
                JobEvent::Settled { job, cause, .. } if job.id == id => Some(*cause),
                _ => None,
            })
            .collect();
        assert_eq!(settled, vec![JobSettleCause::Kill], "round {round}");
        // `stopping` never follows the end of the job.
        let order: Vec<_> = lock(&events)
            .iter()
            .filter(|event| match event {
                JobEvent::Stopping { job } | JobEvent::Settled { job, .. } => job.id == id,
                _ => false,
            })
            .map(event_type)
            .collect();
        assert!(
            order
                .iter()
                .position(|t| *t == "stopping")
                .is_none_or(|stopping| {
                    order
                        .iter()
                        .position(|t| *t == "settled")
                        .is_none_or(|settled| stopping < settled)
                }),
            "round {round}: {order:?}"
        );
    }
}

/// A producer thread writes while `registered` is still being announced: the observer sees
/// `registered` first, and learns of the bytes afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registered_is_the_first_event_even_when_a_producer_thread_writes_during_it() {
    let world = harness(json!({})).await;
    let (go, go_rx) = mpsc::channel::<()>();
    let (wrote, wrote_rx) = mpsc::channel::<()>();
    let go = Mutex::new(Some(go));
    let wrote_rx = Mutex::new(wrote_rx);
    // First listener: blocks the `registered` announcement until the producer thread has written.
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                if matches!(event, JobEvent::Registered { .. })
                    && let Some(go) = lock(&go).take()
                {
                    let _ = go.send(());
                    let _ = lock(&wrote_rx).recv_timeout(Duration::from_secs(3));
                }
                Ok(())
            }),
        )
        .unwrap();
    let events = collect(&world, all_owners(), None);

    let go_rx = Mutex::new(Some(go_rx));
    let spec = JobSpec {
        kind: "bash".into(),
        label: "writer".into(),
        owner: None,
        tool_call: None,
        output_limit_bytes: None,
        output: None,
        run: Box::new(move |job| {
            let go_rx = lock(&go_rx).take().unwrap();
            let wrote = wrote.clone();
            std::thread::spawn(move || {
                let _ = go_rx.recv_timeout(Duration::from_secs(3));
                job.append("from another thread");
                let _ = wrote.send(());
            });
            Ok(JobHooks {
                cancel: Box::new(|_| Ok(())),
                done: Box::pin(std::future::pending()),
            })
        }),
    };
    let id = tokio::task::spawn_blocking({
        let jobs = world.jobs();
        move || jobs.start(spec).unwrap()
    })
    .await
    .unwrap();
    tick().await;
    let seen = types(&events);
    assert_eq!(seen.first(), Some(&"registered"), "{seen:?}");
    assert!(
        seen.contains(&"output"),
        "the write was never announced: {seen:?}"
    );
    assert_eq!(world.jobs().get(&id, None).unwrap().output.total, 19);
}

/// Two starts of one owner at limit 1 must not both pass the check while their starters run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_per_owner_limit_holds_across_parallel_starts() {
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let entered = Arc::new((Mutex::new(0usize), Condvar::new()));
    let make = |entered: Arc<(Mutex<usize>, Condvar)>| {
        let mut p = owned_by(&owner);
        let mut spec = p.spec();
        let inner = spec.run;
        spec.run = Box::new(move |job| {
            // Hold the starter open until the other start has also reached its own, or give up:
            // with the limit enforced the other one never gets here.
            let (count, changed) = &*entered;
            let mut count = lock(count);
            *count += 1;
            changed.notify_all();
            let _ = changed
                .wait_timeout_while(count, Duration::from_millis(400), |count| *count < 2)
                .unwrap();
            inner(job)
        });
        spec
    };
    let results = {
        let (a, b) = (make(entered.clone()), make(entered.clone()));
        let (jobs_a, jobs_b) = (world.jobs(), world.jobs());
        let a = tokio::task::spawn_blocking(move || jobs_a.start(a));
        let b = tokio::task::spawn_blocking(move || jobs_b.start(b));
        [a.await.unwrap(), b.await.unwrap()]
    };
    let accepted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(accepted, 1, "{results:?}");
    let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_job_error(refused, "JOB_LIMIT_REACHED");
    // The refused start left no claim behind: once the first job ends, the owner can start again.
    let live = world.jobs().list(Some(&owner.id()));
    assert_eq!(live.len(), 1);
}

/// A failing starter gives its claimed slot back.
#[tokio::test]
async fn a_failed_starter_does_not_leak_its_slot() {
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let mut failing = owned_by(&owner).spec();
    failing.run = Box::new(|_| Err("starter failed".into()));
    assert!(world.jobs().start(failing).is_err());
    let mut next = owned_by(&owner);
    assert!(world.jobs().start(next.spec()).is_ok());
}

#[tokio::test]
async fn a_wait_beyond_any_timer_is_an_error_not_a_panic() {
    let world = harness(json!({})).await;
    let mut live = producer();
    let id = start_ok(&world, live.spec());
    for timeout in [1e300, 1e22, 2_147_483_648.0] {
        let error = world
            .jobs()
            .wait(&id, timeout, None, None)
            .await
            .expect_err("a live job cannot wait that long");
        assert!(
            error.to_string().contains(
                "deadline timeoutMs must be a positive finite number no greater than 2147483647"
            ),
            "{error}"
        );
    }
    // A settled job needs no timer at all.
    live.settle(completed());
    tick().await;
    assert_eq!(
        world
            .jobs()
            .wait(&id, 1e300, None, None)
            .await
            .unwrap()
            .status,
        JobStatus::Completed
    );
}

/// Dropping a wait without polling it takes its registration with it: the settlement is not
/// counted as collected by anyone.
#[tokio::test]
async fn a_dropped_wait_leaves_no_waiter_behind() {
    let world = bare();
    let registry = direct_registry(&world);
    let mut p = producer();
    let id = registry.start(p.spec()).unwrap();
    let awaited = {
        let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
        let sink = seen.clone();
        registry
            .events()
            .subscribe(
                &world.ctx,
                all_owners(),
                Arc::new(move |event| {
                    if let JobEvent::Settled { awaited, .. } = event {
                        lock(&sink).push(*awaited);
                    }
                    Ok(())
                }),
            )
            .unwrap();
        seen
    };
    let controller = AbortController::new();
    let pending = registry.wait(&id, 60_000.0, None, Some(controller.signal()));
    assert_eq!(registry.wait_resolver_count(&id), 1);
    drop(pending);
    assert_eq!(registry.wait_resolver_count(&id), 0);
    p.settle(completed());
    tick().await;
    assert_eq!(*lock(&awaited), vec![false]);
}
