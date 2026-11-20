//! Port of `tests/jobs.spec.ts` (and the config case of `tests/loader-composition.spec.ts`).

mod common;

use common::*;
use hoh_jobs::{JobChannel, JobChunk, JobEvent, JobSettleCause, JobStatus, JobView};
use hoh_jobs_local::{Config, LocalJobRegistry, LocalJobRegistryPlugin};
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope, scope_of};
use hoh_session::SessionId;
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};

fn chunk(at: u64, text: &str, channel: Option<JobChannel>) -> JobChunk {
    JobChunk {
        at,
        text: text.to_owned(),
        channel,
        gap_before: false,
    }
}

fn scope_in(world: &World) -> hoh_scope::Scope {
    create_scope(&world.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap()
}

// ---------------------------------------------------------------- LocalJobRegistry.start

#[tokio::test]
async fn refuses_to_register_while_no_job_controller_serves_the_owner() {
    let world = jobs_only().await;
    let error = start_err(&world, producer().spec());
    assert!(error.to_string().contains(
        "background jobs unavailable: no job controller serves this agent (load @deepseek-ai/dsh-tool-jobs in its composition)"
    ));
    assert_job_error(
        &start_err(&world, producer().spec()),
        "JOB_CONTROLLER_UNAVAILABLE",
    );
}

#[tokio::test]
async fn refuses_an_owner_whose_own_composition_attaches_no_controller() {
    let world = harness_without_controller(json!({})).await;
    // Two standing preset mounts over one registry; only the first loads the job controls. The
    // second must not inherit the first's open gate.
    let with_controls = scope_in(&world);
    let without_controls = scope_in(&world);
    attach_controller_in(&world, &with_controls.ctx);

    let served = live_agent(&world.ctx, "served", scope_of(&with_controls.ctx)).await;
    let unserved = live_agent(&world.ctx, "unserved", scope_of(&without_controls.ctx)).await;

    start_ok(&world, owned_by(&served).spec());
    assert!(
        start_err(&world, owned_by(&unserved).spec())
            .to_string()
            .contains("no job controller serves this agent")
    );
    // An unowned producer has no chain to walk, so only a global controller serves it.
    assert!(
        start_err(&world, producer().spec())
            .to_string()
            .contains("no job controller serves this agent")
    );
}

#[tokio::test]
async fn lets_a_controller_attached_without_a_scope_serve_every_owner() {
    let world = harness_without_controller(json!({})).await;
    // The host-plane composition's own controls: no scope, so the global layer holds them and
    // every owner's read includes it.
    attach_controller_in(&world, &world.ctx);
    let scoped = live_agent(&world.ctx, "scoped", scope_of(&scope_in(&world).ctx)).await;

    start_ok(&world, owned_by(&scoped).spec());
    start_ok(&world, producer().spec());
}

#[tokio::test]
async fn rejects_an_empty_kind_empty_label_and_invalid_output_limit() {
    let world = harness(json!({})).await;
    let kind = ProducerOptions {
        kind: Some(""),
        ..ProducerOptions::default()
    };
    assert!(
        start_err(&world, producer_with(kind).spec())
            .to_string()
            .contains("invalid job kind")
    );
    let label = ProducerOptions {
        label: Some(""),
        ..ProducerOptions::default()
    };
    assert!(
        start_err(&world, producer_with(label).spec())
            .to_string()
            .contains("invalid job label")
    );
    let limit = ProducerOptions {
        output_limit_bytes: Some(0),
        ..ProducerOptions::default()
    };
    assert!(
        start_err(&world, producer_with(limit).spec())
            .to_string()
            .contains("outputLimitBytes")
    );
}

/// Configuration values the schema (or the `u64` type) must refuse. The source also lists 1.5,
/// NaN and Infinity; a JSON number that is not an unsigned integer is refused by the type, and
/// NaN/Infinity have no JSON form.
#[tokio::test]
async fn rejects_invalid_max_concurrent_jobs_per_owner_config() {
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9_007_199_254_740_992u64),
        json!(null),
        json!("3"),
    ] {
        let world = bare();
        let rejected = match world
            .ctx
            .plugin::<LocalJobRegistryPlugin>(json!({ "maxConcurrentJobsPerOwner": value }))
        {
            Err(_) => true,
            Ok(fiber) => fiber.settled().await.is_err(),
        };
        assert!(rejected, "accepted maxConcurrentJobsPerOwner = {value}");
    }
}

#[tokio::test]
async fn rejects_a_non_positive_pump_interval() {
    let world = bare();
    let rejected = match world
        .ctx
        .plugin::<LocalJobRegistryPlugin>(json!({ "pumpPollMs": 0 }))
    {
        Err(_) => true,
        Ok(fiber) => fiber.settled().await.is_err(),
    };
    assert!(rejected);
}

#[tokio::test]
async fn accepts_the_largest_safe_integer_limit() {
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 9_007_199_254_740_991u64 })).await;
    assert!(world.ctx.get::<hoh_jobs::Jobs>().is_some());
}

#[tokio::test]
async fn defaults_each_owner_bucket_to_ten_active_jobs() {
    let world = harness(json!({})).await;
    let live: Vec<Producer> = (0..10).map(|_| producer()).collect();
    let mut live = live;
    for job in &mut live {
        start_ok(&world, job.spec());
    }

    let (spec, calls) = counted(producer().spec());
    let error = start_err(&world, spec);
    assert!(
        error
            .to_string()
            .contains("background job limit reached for this owner (limit: 10)")
    );
    assert_job_error(&error, "JOB_LIMIT_REACHED");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for job in &live {
        job.settle(completed());
    }
}

#[tokio::test]
async fn rejects_before_producer_start_and_id_allocation_then_admits_immediately_after_settlement()
{
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
    let mut first = producer();
    assert!(is_job_id("bash", &start_ok(&world, first.spec())));

    let mut blocked = producer();
    let (spec, calls) = counted(blocked.spec());
    assert!(
        start_err(&world, spec)
            .to_string()
            .contains("use job_kill to stop an unneeded job, wait for it to finish, then retry")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    first.settle(completed());
    tick().await;
    let mut again = producer();
    assert!(is_job_id("bash", &start_ok(&world, again.spec())));
}

#[tokio::test]
async fn keeps_a_stopping_job_in_the_bucket_until_producer_settlement() {
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
    let mut first = producer();
    let id = start_ok(&world, first.spec());
    assert_eq!(
        world.jobs().kill(&id, None, None).unwrap(),
        hoh_jobs::KillOutcome::Requested
    );

    assert!(
        start_err(&world, producer().spec())
            .to_string()
            .contains("(limit: 1)")
    );

    first.settle(killed());
    tick().await;
    assert!(is_job_id("bash", &start_ok(&world, producer().spec())));
}

#[tokio::test]
async fn releases_the_bucket_after_a_terminal_outcome() {
    for status in [
        hoh_jobs::JobOutcomeStatus::Completed,
        hoh_jobs::JobOutcomeStatus::Killed,
        hoh_jobs::JobOutcomeStatus::Failed,
    ] {
        let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
        let mut first = producer();
        start_ok(&world, first.spec());
        first.settle(outcome(status));
        tick().await;
        start_ok(&world, producer().spec());
    }
}

#[tokio::test]
async fn buckets_by_exact_owner_a_same_session_replacement_and_the_unowned_bucket_count_separately()
{
    let world = harness(json!({ "maxConcurrentJobsPerOwner": 1 })).await;
    let old_owner = stub_agent(&world.ctx, "shared-session", None);
    let detach_old = register_agent(&world.ctx, &old_owner).await;
    let mut old_task = owned_by(&old_owner);
    start_ok(&world, old_task.spec());

    live_agent(&world.ctx, "other-session", None).await;
    let mut other = producer();
    other.spec.as_mut().unwrap().owner = Some(SessionId::from("other-session"));
    start_ok(&world, other.spec());

    // The session id resolves to whichever agent is live now; a replacement instance starts with
    // its own bucket even while the old job runs on.
    detach_old.dispose().await;
    let replacement = stub_agent(&world.ctx, "shared-session", None);
    register_agent(&world.ctx, &replacement).await;
    start_ok(&world, owned_by(&replacement).spec());

    start_ok(&world, producer().spec());
    assert!(
        start_err(&world, producer().spec())
            .to_string()
            .contains("(limit: 1)")
    );

    old_task.settle(completed());
    tick().await;
    old_owner.dispose_scope().await;
}

#[tokio::test]
async fn issues_unique_kind_prefixed_ids() {
    let world = harness(json!({})).await;
    let first = start_ok(&world, producer().spec());
    let second = start_ok(&world, producer().spec());
    assert!(is_job_id("bash", &first));
    assert!(is_job_id("bash", &second));
    assert_ne!(first, second);
    let kind = |kind| ProducerOptions {
        kind: Some(kind),
        ..ProducerOptions::default()
    };
    assert!(is_job_id(
        "test-task",
        &start_ok(&world, producer_with(kind("test-task")).spec())
    ));
    assert!(is_job_id(
        "workflow",
        &start_ok(&world, producer_with(kind("workflow")).spec())
    ));
}

#[tokio::test]
async fn retries_a_short_uuid_collision_without_replacing_a_live_job() {
    use hoh_jobs::JobRegistryApi;
    let world = bare();
    let script = std::sync::Mutex::new(
        [
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "aaaaaaaa-aaaa-4aaa-8aaa-bbbbbbbbbbbb",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::VecDeque<_>>(),
    );
    let registry = LocalJobRegistry::with_uuid_source(
        world.ctx.clone(),
        Config::default(),
        Arc::new(move || lock(&script).pop_front().expect("uuid script exhausted")),
    )
    .unwrap();
    registry
        .attach_controller(&world.ctx, "test-controller")
        .unwrap();
    let first = registry.start(producer().spec()).unwrap();
    let second = registry.start(producer().spec()).unwrap();
    assert_eq!(first.to_string(), "bash-aaaaaaaaaaaa4aaa");
    assert_eq!(second.to_string(), "bash-bbbbbbbbbbbb4bbb");
    let listed: Vec<JobId> = registry.list(None).into_iter().map(|job| job.id).collect();
    assert_eq!(listed, vec![first, second]);
}

use hoh_jobs::JobId;

// ------------------------------------------------- LocalJobRegistry reads and settlement

#[tokio::test]
async fn read_consumes_the_ring_from_the_model_cursor_and_advances_it_read_at_never_moves_it() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let jobs = world.jobs();
    let id = start_ok(&world, p.spec());
    p.job().append_with_options(
        "first",
        hoh_jobs::JobAppendOptions {
            channel: Some(JobChannel::Stdout),
            gap_before: false,
        },
    );
    p.job().append_with_options(
        "err",
        hoh_jobs::JobAppendOptions {
            channel: Some(JobChannel::Stderr),
            gap_before: false,
        },
    );

    let first = jobs.read(&id, None).unwrap();
    assert_eq!(
        first.chunks,
        vec![
            chunk(0, "first", Some(JobChannel::Stdout)),
            chunk(5, "err", Some(JobChannel::Stderr))
        ]
    );
    assert!(!first.lossy);
    assert_eq!(first.result, None);
    assert_eq!(first.job.status, JobStatus::Running);
    assert_eq!((first.job.output.total, first.job.output.earliest), (8, 0));
    assert_eq!(jobs.read(&id, None).unwrap().chunks, vec![]);

    // A non-consuming read from the head sees everything again, and leaves the cursor alone.
    assert_eq!(jobs.read_at(&id, 0.0, None).unwrap().chunks.len(), 2);
    p.job().append("rest");
    assert_eq!(
        jobs.read(&id, None).unwrap().chunks,
        vec![chunk(8, "rest", None)]
    );
}

#[tokio::test]
async fn hands_the_producer_result_out_once_on_the_first_read_after_settlement() {
    let world = harness(json!({})).await;
    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("research job"),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    let live = jobs.read(&id, None).unwrap();
    assert!(live.chunks.is_empty());
    assert_eq!(live.job.status, JobStatus::Running);

    p.settle(hoh_jobs::JobOutcome {
        result: Some("final answer".to_owned()),
        ..completed()
    });
    tick().await;
    let read = jobs.read(&id, None).unwrap();
    assert_eq!(read.result.as_deref(), Some("final answer"));
    assert_eq!(read.job.status, JobStatus::Completed);
    assert!(read.job.finished_at.is_some());
    assert_eq!(jobs.read(&id, None).unwrap().result, None);
}

#[tokio::test]
async fn a_settled_job_without_a_result_reads_as_empty() {
    let world = harness(json!({})).await;
    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    p.settle(failed("max-tokens"));
    tick().await;
    let read = world.jobs().read(&id, None).unwrap();
    assert!(read.chunks.is_empty());
    assert_eq!(read.job.status, JobStatus::Failed);
    assert_eq!(read.job.detail.as_deref(), Some("max-tokens"));
}

#[tokio::test]
async fn projects_a_producer_owned_model_output_limit_into_views() {
    let world = harness(json!({})).await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(64),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    assert_eq!(
        world.jobs().get(&id, None).unwrap().output_limit_bytes,
        Some(64)
    );
    assert_eq!(
        world.jobs().read(&id, None).unwrap().job.output_limit_bytes,
        Some(64)
    );
}

#[tokio::test]
async fn throws_for_unknown_job_ids() {
    let world = harness(json!({})).await;
    let missing = job_id("bash-99");
    let jobs = world.jobs();
    assert!(
        jobs.read(&missing, None)
            .unwrap_err()
            .to_string()
            .contains("unknown job bash-99")
    );
    let error = jobs.get(&missing, None).unwrap_err();
    assert!(error.to_string().contains("unknown job bash-99"));
    assert_job_error(&error, "JOB_NOT_FOUND");
}

#[tokio::test]
async fn announces_settlement_once_with_containment_across_listeners() {
    let world = harness(json!({})).await;
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(|_| Err("listener boom".into())),
        )
        .unwrap();
    let seen = collect(&world, all_owners(), Some(&["settled"]));

    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    tick().await;

    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    let JobEvent::Settled { job, cause, .. } = &events[0] else {
        panic!("expected settled, got {:?}", events[0]);
    };
    assert_eq!(*cause, JobSettleCause::Producer);
    assert_eq!(job.id, id);
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(job.detail.as_deref(), Some("exit code: 0"));
    assert!(world.warnings().iter().any(|w| w.contains("listener boom")));
}

#[tokio::test]
async fn contains_a_failing_done_future_as_a_failed_outcome_producer_contract_violation() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.reject("transport exploded");
    tick().await;

    let view: JobView = world.jobs().get(&id, None).unwrap();
    assert_eq!(view.status, JobStatus::Failed);
    assert_eq!(view.detail.as_deref(), Some("Error: transport exploded"));
    assert!(
        world
            .warnings()
            .iter()
            .any(|w| w.contains("producer contract violation"))
    );
}

#[tokio::test]
async fn unregisters_event_listeners_with_the_contributing_fiber_hmr_safety() {
    let world = harness(json!({})).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let plugin_ctx = world.ctx.child("plugin").unwrap();
    let sink = seen.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &plugin_ctx,
            all_owners(),
            Arc::new(move |event| {
                lock(&sink).push(event_type(event));
                Ok(())
            }),
        )
        .unwrap();
    plugin_ctx.fiber().dispose().await;
    // The returned disposer detaches too (the non-fiber path).
    let sink = seen.clone();
    let detach = world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                lock(&sink).push(event_type(event));
                Ok(())
            }),
        )
        .unwrap();
    detach.dispose().await;

    let mut p = producer();
    start_ok(&world, p.spec());
    p.settle(completed());
    tick().await;
    assert!(lock(&seen).is_empty());
}

// ---------------------------------------------------------------- LocalJobRegistry.kill

use hoh_jobs::{JobRegistryApi, KillOutcome};
use hoh_timeout::{AbortController, is_abort_error};

fn event_job(event: &JobEvent) -> &JobView {
    match event {
        JobEvent::Registered { job }
        | JobEvent::Progress { job }
        | JobEvent::Stopping { job }
        | JobEvent::Removed { job }
        | JobEvent::Settled { job, .. } => job,
        JobEvent::Output { .. } => panic!("output event has no job"),
    }
}

fn settled_cause(event: &JobEvent) -> JobSettleCause {
    match event {
        JobEvent::Settled { cause, .. } => *cause,
        other => panic!("expected settled, got {other:?}"),
    }
}

#[tokio::test]
async fn cancels_a_live_job_with_the_forwarded_reason_and_settles_with_cause_kill() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["stopping", "settled"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());

    assert_eq!(
        world
            .jobs()
            .kill(&id, None, Some("no longer needed"))
            .unwrap(),
        KillOutcome::Requested
    );
    assert_eq!(p.cancels(), vec![Some("no longer needed".to_owned())]);
    assert_eq!(world.jobs().list(None)[0].status, JobStatus::Stopping);
    assert_eq!(types(&seen), vec!["stopping"]);
    assert_eq!(event_job(&lock(&seen)[0]).id, id);

    p.settle(hoh_jobs::JobOutcome {
        detail: Some("signal: SIGTERM".to_owned()),
        ..killed()
    });
    tick().await;
    let events = lock(&seen).clone();
    assert_eq!(settled_cause(&events[1]), JobSettleCause::Kill);
    let job = event_job(&events[1]);
    assert_eq!((job.id.clone(), job.status), (id, JobStatus::Killed));
    assert_eq!(
        job.detail.as_deref(),
        Some("signal: SIGTERM; no longer needed")
    );
}

#[tokio::test]
async fn merges_a_reason_without_producer_detail_and_keeps_producer_detail_alone_when_the_job_outran_the_kill()
 {
    let world = harness(json!({})).await;
    let mut cancelled = producer();
    let killed_id = start_ok(&world, cancelled.spec());
    world
        .jobs()
        .kill(&killed_id, None, Some("cancelled by the user"))
        .unwrap();
    cancelled.settle(killed());
    let mut outran = producer();
    let outran_id = start_ok(&world, outran.spec());
    world
        .jobs()
        .kill(&outran_id, None, Some("too late"))
        .unwrap();
    outran.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    tick().await;
    assert_eq!(
        world
            .jobs()
            .get(&killed_id, None)
            .unwrap()
            .detail
            .as_deref(),
        Some("cancelled by the user")
    );
    assert_eq!(
        world
            .jobs()
            .get(&outran_id, None)
            .unwrap()
            .detail
            .as_deref(),
        Some("exit code: 0")
    );
}

#[tokio::test]
async fn reports_an_already_finished_job_instead_of_failing() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.settle(completed());
    tick().await;
    assert_eq!(
        world.jobs().kill(&id, None, None).unwrap(),
        KillOutcome::AlreadyFinished
    );
}

#[tokio::test]
async fn propagates_a_failing_producer_cancel_and_leaves_the_job_untouched() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["stopping", "settled"]));
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = broken.clone();
    let mut p = producer_with(ProducerOptions {
        label: Some("flaky cancel"),
        cancel: Some(Box::new(move |_| {
            if flag.load(Ordering::SeqCst) {
                Err("cancel boom".into())
            } else {
                Ok(())
            }
        })),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    assert!(
        world
            .jobs()
            .kill(&id, None, None)
            .unwrap_err()
            .to_string()
            .contains("cancel boom")
    );
    // The failed kill mutated NOTHING: still running, and the settlement still reports the
    // producer as its cause.
    assert_eq!(
        world.jobs().get(&id, None).unwrap().status,
        JobStatus::Running
    );
    assert!(lock(&seen).is_empty());
    p.settle(completed());
    tick().await;
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(settled_cause(&events[0]), JobSettleCause::Producer);
    assert_eq!(event_job(&events[0]).id, id);

    broken.store(false, Ordering::SeqCst);
    assert_eq!(
        world.jobs().kill(&id, None, None).unwrap(),
        KillOutcome::AlreadyFinished
    );
}

// ----------------------------------------------------------------- LocalJobRegistry.wait

#[tokio::test]
async fn wait_resolves_with_the_terminal_projection_when_the_job_settles_before_the_settled_event()
{
    let world = harness(json!({})).await;
    let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let sink = order.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                if matches!(event, JobEvent::Settled { .. }) {
                    lock(&sink).push("event");
                }
                Ok(())
            }),
        )
        .unwrap();
    let mut p = producer();
    let id = start_ok(&world, p.spec());

    let pending = world.jobs().wait(&id, 5_000.0, None, None);
    let sink = order.clone();
    let wait = tokio::spawn(async move {
        let view = pending.await;
        lock(&sink).push("wait");
        view
    });
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    assert_eq!(wait.await.unwrap().unwrap().status, JobStatus::Completed);
    // Waiters are released synchronously inside the settlement, ahead of the event; their
    // continuations run afterwards, which is why a claim made when the wait starts (not when it
    // returns) is what stays ordered.
    assert_eq!(*lock(&order), vec!["event", "wait"]);
}

#[tokio::test]
async fn wait_returns_the_live_projection_on_timeout() {
    let world = harness(json!({})).await;
    let id = start_ok(&world, producer().spec());
    let view = world.jobs().wait(&id, 5.0, None, None).await.unwrap();
    assert_eq!(view.status, JobStatus::Running);
}

#[tokio::test]
async fn unregisters_timed_out_and_aborted_wait_resolvers_while_the_job_remains_live() {
    let world = bare();
    let registry = direct_registry(&world);
    let id = registry.start(producer().spec()).unwrap();

    for _ in 0..3 {
        let wait = registry.wait(&id, 5.0, None, None);
        assert_eq!(registry.wait_resolver_count(&id), 1);
        assert_eq!(wait.await.unwrap().status, JobStatus::Running);
        assert_eq!(registry.wait_resolver_count(&id), 0);
    }

    let controller = AbortController::new();
    let wait = registry.wait(&id, 5_000.0, None, Some(controller.signal()));
    assert_eq!(registry.wait_resolver_count(&id), 1);
    controller.abort();
    assert!(wait.await.unwrap_err().to_string().contains("wait aborted"));
    assert_eq!(registry.wait_resolver_count(&id), 0);
    assert_eq!(registry.get(&id, None).unwrap().status, JobStatus::Running);
}

#[tokio::test]
async fn wait_returns_immediately_for_an_already_finished_job() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.settle(completed());
    tick().await;
    assert_eq!(
        world
            .jobs()
            .wait(&id, 5_000.0, None, None)
            .await
            .unwrap()
            .status,
        JobStatus::Completed
    );
}

#[tokio::test]
async fn wait_rejects_a_non_positive_or_non_finite_timeout() {
    let world = harness(json!({})).await;
    let id = start_ok(&world, producer().spec());
    for timeout in [0.0, f64::NAN] {
        let error = world
            .jobs()
            .wait(&id, timeout, None, None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("invalid wait timeout"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn an_aborted_signal_rejects_the_wait_only_the_job_stays_alive() {
    let world = harness(json!({})).await;
    let id = start_ok(&world, producer().spec());

    let controller = AbortController::new();
    let wait = world
        .jobs()
        .wait(&id, 5_000.0, None, Some(controller.signal()));
    controller.abort();
    let error = wait.await.unwrap_err();
    assert!(error.to_string().contains("wait aborted"));
    assert!(is_abort_error(error.as_ref()));
    assert_eq!(world.jobs().list(None)[0].status, JobStatus::Running);

    let pre_aborted = AbortController::new();
    pre_aborted.abort();
    for _ in 0..2 {
        let error = world
            .jobs()
            .wait(&id, 5_000.0, None, Some(pre_aborted.signal()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("wait aborted"));
        assert!(is_abort_error(error.as_ref()));
    }
}

#[tokio::test]
async fn an_abort_racing_settlement_in_the_same_tick_rejects_the_wait_and_leaves_the_settlement_intact()
 {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());

    let controller = AbortController::new();
    let wait = world
        .jobs()
        .wait(&id, 5_000.0, None, Some(controller.signal()));
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    controller.abort();
    assert!(wait.await.unwrap_err().to_string().contains("wait aborted"));
    tick().await;
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(settled_cause(&events[0]), JobSettleCause::Producer);
    assert_eq!(event_job(&events[0]).status, JobStatus::Completed);
}

#[tokio::test]
async fn an_abort_landing_after_settlement_still_delivers_the_terminal_projection_it_owes() {
    let world = harness(json!({})).await;
    let controller = Arc::new(AbortController::new());
    // The listener aborts after settlement released this waiter but before its continuation
    // runs. Releasing waiters ahead of the announcement is what makes that abort harmless; this
    // is the guard on that ordering.
    let abort = controller.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                if matches!(event, JobEvent::Settled { .. }) {
                    abort.abort();
                }
                Ok(())
            }),
        )
        .unwrap();
    let mut p = producer();
    let id = start_ok(&world, p.spec());

    let wait = world
        .jobs()
        .wait(&id, 5_000.0, None, Some(controller.signal()));
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    assert_eq!(wait.await.unwrap().status, JobStatus::Completed);
}

// ------------------------------------------------ LocalJobRegistry settled event awaited flag

fn awaited(event: &JobEvent) -> bool {
    match event {
        JobEvent::Settled { awaited, .. } => *awaited,
        other => panic!("expected settled, got {other:?}"),
    }
}

#[tokio::test]
async fn reports_a_settlement_that_released_a_live_wait_as_awaited() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let wait = world.jobs().wait(&id, 5_000.0, None, None);
    p.settle(completed());
    wait.await.unwrap();
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert!(awaited(&events[0]));
}

#[tokio::test]
async fn reports_a_settlement_nobody_waited_on_or_whose_waits_timed_out_or_aborted_as_not_awaited()
{
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));
    let mut unwatched = producer();
    let unwatched_id = start_ok(&world, unwatched.spec());
    unwatched.settle(completed());
    tick().await;

    let mut abandoned = producer();
    let abandoned_id = start_ok(&world, abandoned.spec());
    world
        .jobs()
        .wait(&abandoned_id, 5.0, None, None)
        .await
        .unwrap();
    let controller = AbortController::new();
    let aborted = world
        .jobs()
        .wait(&abandoned_id, 5_000.0, None, Some(controller.signal()));
    controller.abort();
    assert!(
        aborted
            .await
            .unwrap_err()
            .to_string()
            .contains("wait aborted")
    );
    abandoned.settle(completed());
    tick().await;
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (&event_job(&events[0]).id, awaited(&events[0])),
        (&unwatched_id, false)
    );
    assert_eq!(
        (&event_job(&events[1]).id, awaited(&events[1])),
        (&abandoned_id, false)
    );
}

#[tokio::test]
async fn counts_a_wait_still_owed_the_projection_even_when_another_wait_on_the_job_timed_out() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let patient = world.jobs().wait(&id, 5_000.0, None, None);
    world.jobs().wait(&id, 5.0, None, None).await.unwrap();
    p.settle(completed());
    patient.await.unwrap();
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert!(awaited(&events[0]));
}

// ---------------------------------------------------------------- LocalJobRegistry.remove

#[tokio::test]
async fn remove_drops_a_settled_record_from_the_visible_set_and_announces_the_removal() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["removed"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    tick().await;
    world.jobs().remove(&id, None).unwrap();
    assert!(world.jobs().list(None).is_empty());
    assert!(
        world
            .jobs()
            .get(&id, None)
            .unwrap_err()
            .to_string()
            .contains(&format!("unknown job {id}"))
    );
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    let job = event_job(&events[0]);
    assert_eq!((&job.id, job.status), (&id, JobStatus::Completed));
    assert_eq!(job.detail.as_deref(), Some("exit code: 0"));
}

#[tokio::test]
async fn remove_refuses_a_live_job_an_unknown_id_and_a_foreign_caller() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let mut p = owned_by(&owner);
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    assert!(
        jobs.remove(&id, Some(&owner.id()))
            .unwrap_err()
            .to_string()
            .contains(&format!("job {id} is still running"))
    );
    assert!(
        jobs.remove(&id, None)
            .unwrap_err()
            .to_string()
            .contains("belongs to another session")
    );
    assert!(
        jobs.remove(&job_id("bash-99"), Some(&owner.id()))
            .unwrap_err()
            .to_string()
            .contains("unknown job bash-99")
    );
    p.settle(completed());
    tick().await;
    jobs.remove(&id, Some(&owner.id())).unwrap();
    assert!(jobs.list(Some(&owner.id())).is_empty());
}

// ------------------------------------------------------- LocalJobRegistry owner isolation

#[tokio::test]
async fn fences_every_view_operation_to_the_owning_session_and_keeps_unowned_jobs_open() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let other = stub_agent(&world.ctx, "other", None);

    let jobs = world.jobs();
    let owned = start_ok(&world, owned_by(&owner).spec());
    let open = start_ok(&world, producer().spec());

    // The owner and the unowned job are reachable.
    assert_eq!(jobs.read(&owned, Some(&owner.id())).unwrap().job.id, owned);
    assert_eq!(jobs.read(&open, Some(&other.id())).unwrap().job.id, open);

    // A different session and an anonymous caller are rejected.
    let foreign = Some(other.id());
    let message = format!("job {owned} belongs to another session");
    assert!(
        jobs.read(&owned, foreign.as_ref())
            .unwrap_err()
            .to_string()
            .contains(&message)
    );
    assert!(
        jobs.get(&owned, foreign.as_ref())
            .unwrap_err()
            .to_string()
            .contains("belongs to another session")
    );
    assert!(
        jobs.read_at(&owned, 0.0, foreign.as_ref())
            .unwrap_err()
            .to_string()
            .contains("belongs to another session")
    );
    assert!(
        jobs.kill(&owned, foreign.as_ref(), None)
            .unwrap_err()
            .to_string()
            .contains("belongs to another session")
    );
    assert!(
        jobs.wait(&owned, 10.0, foreign.as_ref(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("belongs to another session")
    );
    let anonymous = jobs.read(&owned, None).unwrap_err();
    assert!(anonymous.to_string().contains("belongs to another session"));
    assert_job_error(&anonymous, "JOB_FOREIGN_SESSION");
}

#[tokio::test]
async fn list_shows_only_caller_owned_plus_unowned_jobs() {
    let world = harness(json!({})).await;
    let alice = live_agent(&world.ctx, "alice", None).await;
    let bob = live_agent(&world.ctx, "bob", None).await;

    let alice_task = start_ok(&world, owned_by(&alice).spec());
    let bob_task = start_ok(&world, owned_by(&bob).spec());
    let open_task = start_ok(
        &world,
        producer_with(ProducerOptions {
            kind: Some("test-task"),
            ..ProducerOptions::default()
        })
        .spec(),
    );

    let ids = |caller: Option<&SessionId>| -> Vec<JobId> {
        world
            .jobs()
            .list(caller)
            .into_iter()
            .map(|job| job.id)
            .collect()
    };
    assert_eq!(ids(Some(&alice.id())), vec![alice_task, open_task.clone()]);
    assert_eq!(ids(Some(&bob.id())), vec![bob_task, open_task.clone()]);
    assert_eq!(ids(None), vec![open_task]);
}

#[tokio::test]
async fn rejects_an_owned_registration_when_no_agent_registry_is_mounted() {
    let world = jobs_only().await;
    world
        .jobs()
        .attach_controller(&world.ctx, "test-controller")
        .unwrap();
    let owned = || {
        let mut p = producer();
        let mut spec = p.spec();
        spec.owner = Some(SessionId::from("a"));
        spec
    };
    assert!(
        start_err(&world, owned())
            .to_string()
            .contains("background job ownership requires the agent registry")
    );
    assert_job_error(
        &start_err(&world, owned()),
        "JOB_OWNER_REGISTRY_UNAVAILABLE",
    );
    // The failed registration mutated nothing: no stored job.
    assert!(world.jobs().list(None).is_empty());
    assert!(is_job_id("bash", &start_ok(&world, producer().spec())));
}

#[tokio::test]
async fn rejects_a_session_without_a_live_agent_and_attaches_cleanup_once_it_has_one() {
    let world = harness(json!({})).await;
    let ghost = stub_agent(&world.ctx, "ghost", None); // never registered in the agent registry

    // Owner resolution precedes registry mutation and cleanup attachment.
    assert!(
        start_err(&world, owned_by(&ghost).spec())
            .to_string()
            .contains("has no live agent")
    );
    assert_job_error(
        &start_err(&world, owned_by(&ghost).spec()),
        "JOB_OWNER_NOT_LIVE",
    );
    assert!(world.jobs().list(Some(&ghost.id())).is_empty());

    // A later valid registration must still attach cleanup for the live instance.
    register_agent(&world.ctx, &ghost).await;
    let mut p = producer_with(ProducerOptions {
        label: Some("after retry"),
        owner: Some(ghost.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    assert!(is_job_id("bash", &id));
    ghost.dispose_scope().await;
    assert_eq!(p.cancels(), vec![Some("owner disposed".to_owned())]);
    assert!(world.jobs().list(Some(&ghost.id())).is_empty());
}

#[tokio::test]
async fn binds_a_session_id_to_the_agent_that_is_live_now_and_reads_stay_keyed_by_that_id() {
    let world = harness(json!({})).await;
    let stale_owner = stub_agent(&world.ctx, "owner", None);
    let unregister_stale = register_agent(&world.ctx, &stale_owner).await;
    unregister_stale.dispose().await;
    let current_owner = stub_agent(&world.ctx, "owner", None);
    register_agent(&world.ctx, &current_owner).await;

    let mut current = owned_by(&current_owner);
    let id = start_ok(&world, current.spec());
    // The stale instance shares the id, so the fence lets it read; ownership (and therefore
    // teardown) belongs to the live instance.
    assert_eq!(
        world
            .jobs()
            .get(&id, Some(&stale_owner.id()))
            .unwrap()
            .owner,
        Some(current_owner.id())
    );
    stale_owner.dispose_scope().await;
    assert_eq!(world.jobs().list(Some(&current_owner.id())).len(), 1);

    current.settle(completed());
    tick().await;
    current_owner.dispose_scope().await;
    assert!(world.jobs().list(Some(&current_owner.id())).is_empty());
}

// -------------------------------------------------------- LocalJobRegistry owner cleanup

#[tokio::test]
async fn drains_the_owner_cancels_live_jobs_awaits_settlement_removes_the_rows() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let seen = collect(&world, owner_filter(&owner.id()), Some(&["removed"]));

    // The producer settles only when cancelled — models a child that stops on request.
    let mut research = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("long research"),
        owner: Some(owner.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start_ok(&world, research.spec());
    let mut terminal = owned_by(&owner);
    start_ok(&world, terminal.spec());
    terminal.settle(completed());
    tick().await;

    owner.dispose_scope().await;
    assert_eq!(research.cancels(), vec![Some("owner disposed".to_owned())]);
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
    let removed: Vec<JobId> = lock(&seen)
        .iter()
        .map(|event| event_job(event).id.clone())
        .collect();
    assert_eq!(removed.len(), 2);
    assert!(is_job_id("test-task", &removed[0]), "{removed:?}");
    assert!(is_job_id("bash", &removed[1]), "{removed:?}");
}

#[tokio::test]
async fn announces_teardown_settlements_with_cause_teardown() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));

    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("long research"),
        owner: Some(owner.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());

    owner.dispose_scope().await;
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(settled_cause(&events[0]), JobSettleCause::Teardown);
    assert_eq!(event_job(&events[0]).status, JobStatus::Killed);
}

/// The source counts `jobs.ownerCleanup()` effects on the agent fiber; the kernel exposes no
/// effect list, so the same property is observed through behaviour: one cleanup drains every
/// owned job, cancelling each exactly once.
#[tokio::test]
async fn attaches_one_cleanup_per_owner_and_drains_all_owned_jobs_with_the_scope() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let options = || ProducerOptions {
        owner: Some(owner.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    };
    let mut first = producer_with(options());
    let mut second = producer_with(options());
    start_ok(&world, first.spec());
    start_ok(&world, second.spec());
    let mut third = producer_with(options());
    start_ok(&world, third.spec());
    third.settle(completed());
    tick().await;
    owner.dispose_scope().await;
    assert_eq!(first.cancels(), vec![Some("owner disposed".to_owned())]);
    assert_eq!(second.cancels(), vec![Some("owner disposed".to_owned())]);
    assert!(third.cancels().is_empty());
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
}

#[tokio::test]
async fn does_not_let_an_old_scope_cleanup_cancel_a_same_session_replacement_job() {
    let world = harness(json!({})).await;
    let old_owner = stub_agent(&world.ctx, "owner", None);
    let detach_old = register_agent(&world.ctx, &old_owner).await;
    let by_session = || ProducerOptions {
        owner: Some(SessionId::from("owner")),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    };

    let mut old_job = producer_with(by_session());
    start_ok(&world, old_job.spec());
    detach_old.dispose().await;
    let replacement = stub_agent(&world.ctx, "owner", None);
    register_agent(&world.ctx, &replacement).await;
    let mut replacement_job = producer_with(by_session());
    let replacement_id = start_ok(&world, replacement_job.spec());

    old_owner.dispose_scope().await;
    assert_eq!(old_job.cancels().len(), 1);
    assert!(replacement_job.cancels().is_empty());
    let listed: Vec<JobId> = world
        .jobs()
        .list(Some(&replacement.id()))
        .into_iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(listed, vec![replacement_id]);

    replacement.dispose_scope().await;
    assert_eq!(replacement_job.cancels().len(), 1);
}

/// The source asserts the cleanup effect sits on the agent fiber, not the jobs fiber. Observed
/// through behaviour: disposing the owner scope leaves the service mounted and usable.
#[tokio::test]
async fn registers_owner_cleanup_on_the_agent_scope_rather_than_the_jobs_fiber() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let mut first = owned_by(&owner);
    start_ok(&world, first.spec());
    first.settle(completed());
    tick().await;
    owner.dispose_scope().await;

    // Only the owner registration is released; the long-lived jobs service and its own
    // teardown remain active.
    assert!(world.ctx.get::<hoh_jobs::Jobs>().is_some());
    let mut later = producer();
    assert!(is_job_id("bash", &start_ok(&world, later.spec())));
    later.settle(completed());
    tick().await;
    let fiber = world.jobs_fiber.clone().unwrap();
    let mut live = producer_with(ProducerOptions {
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start_ok(&world, live.spec());
    fiber.dispose().await;
    assert_eq!(
        live.cancels(),
        vec![Some("jobs service disposed".to_owned())]
    );
}

#[tokio::test]
async fn force_fails_a_failing_teardown_cancel_without_awaiting_producer_done_first_outcome_wins() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));

    let mut p = producer_with(ProducerOptions {
        label: Some("broken producer"),
        owner: Some(owner.id()),
        cancel: Some(Box::new(|_| Err("cancel boom".into()))),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());

    let scope = owner.clone();
    let drain = tokio::spawn(async move { scope.dispose_scope().await });
    tick().await;
    let drained_without_producer_done = drain.is_finished();
    // Release the producer either way: a late completion must not replace the failure or notify
    // twice.
    p.settle(completed());
    drain.await.unwrap();
    tick().await;

    assert!(drained_without_producer_done);
    assert!(
        world
            .warnings()
            .iter()
            .any(|w| w.contains("work may be orphaned"))
    );
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(settled_cause(&events[0]), JobSettleCause::Teardown);
    let job = event_job(&events[0]);
    assert_eq!(job.status, JobStatus::Failed);
    assert!(
        job.detail
            .as_deref()
            .unwrap()
            .contains("cancel threw during teardown")
    );
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
}

// ------------------------------------------------------------ LocalJobRegistry disposal

#[tokio::test]
async fn disposal_cancels_live_jobs_awaits_settlement_and_announces_the_teardown_to_outside_subscribers()
 {
    let world = harness(json!({})).await;
    let plugin_ctx = world.ctx.child("plugin").unwrap();
    world
        .jobs()
        .attach_controller(&plugin_ctx, "test-controller")
        .unwrap();

    // A subscriber outside the registry's fiber is still listening when the registry unloads: it
    // must see the rows leave, not keep them forever.
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                lock(&sink).push(match event {
                    JobEvent::Settled { cause, .. } => format!("settled:{}", cause_name(*cause)),
                    other => event_type(other).to_owned(),
                });
                Ok(())
            }),
        )
        .unwrap();
    let mut p = producer_with(ProducerOptions {
        label: Some("sleep 600"),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());

    world.jobs_fiber.clone().unwrap().dispose().await;
    assert_eq!(p.cancels(), vec![Some("jobs service disposed".to_owned())]);
    assert_eq!(
        *lock(&seen),
        vec![
            "registered",
            "stopping",
            "settled:teardown",
            "output",
            "removed"
        ]
    );
}

fn cause_name(cause: JobSettleCause) -> &'static str {
    match cause {
        JobSettleCause::Producer => "producer",
        JobSettleCause::Kill => "kill",
        JobSettleCause::Teardown => "teardown",
    }
}

#[tokio::test]
async fn disposal_force_fails_a_failing_cancel_so_service_disposal_does_not_await_producer_done() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), Some(&["settled"]));
    let mut p = producer_with(ProducerOptions {
        label: Some("broken service job"),
        cancel: Some(Box::new(|_| Err("service cancel boom".into()))),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());

    let fiber = world.jobs_fiber.clone().unwrap();
    let disposal = tokio::spawn(async move { fiber.dispose().await });
    tick().await;
    let disposed_without_producer_done = disposal.is_finished();
    p.settle(completed());
    disposal.await.unwrap();
    tick().await;

    assert!(disposed_without_producer_done);
    assert!(
        world
            .warnings()
            .iter()
            .any(|w| w.contains("work may be orphaned"))
    );
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(settled_cause(&events[0]), JobSettleCause::Teardown);
    assert_eq!(event_job(&events[0]).status, JobStatus::Failed);
}

/// The source counts the agent fiber's `jobs.ownerCleanup()` effects before and after the unload;
/// observed through behaviour: after the service unloads, the live agent's later disposal has
/// nothing left to cancel or announce.
#[tokio::test]
async fn detaches_owner_effects_from_still_live_agent_scopes_when_the_service_unloads() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let mut p = producer_with(ProducerOptions {
        label: Some("owned work"),
        owner: Some(owner.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());
    let seen = collect(&world, all_owners(), Some(&["settled", "removed"]));

    world.jobs_fiber.clone().unwrap().dispose().await;
    assert_eq!(p.cancels().len(), 1);
    let after_unload = seen.lock().unwrap().len();

    owner.dispose_scope().await;
    assert_eq!(p.cancels().len(), 1);
    assert_eq!(seen.lock().unwrap().len(), after_unload);
}

#[tokio::test]
async fn drops_a_scoped_layer_when_its_registrations_dispose() {
    let world = harness_without_controller(json!({})).await;
    let standing = scope_in(&world);
    // One mount contributes both kinds into the same layer, as `tool-jobs` does; unloading it
    // must leave nothing serving the agents that joined it.
    let mount = standing.ctx.child("plugin").unwrap();
    world.jobs().attach_controller(&mount, "tool-jobs").unwrap();
    world
        .jobs()
        .events()
        .subscribe(&mount, scope_owners(), Arc::new(|_| Ok(())))
        .unwrap();
    let owner = live_agent(&world.ctx, "joined", scope_of(&standing.ctx)).await;
    start_ok(&world, owned_by(&owner).spec());

    mount.fiber().dispose().await;

    assert!(
        start_err(&world, owned_by(&owner).spec())
            .to_string()
            .contains("no job controller serves this agent")
    );
}

#[tokio::test]
async fn detaching_the_last_controller_re_arms_the_register_fence() {
    let world = jobs_only().await;
    let detach_a1 = world.jobs().attach_controller(&world.ctx, "a").unwrap();
    let detach_a2 = world.jobs().attach_controller(&world.ctx, "a").unwrap(); // duplicate name counts independently
    let mount = world.ctx.child("plugin").unwrap();
    world.jobs().attach_controller(&mount, "b").unwrap();

    detach_a1.dispose().await;
    start_ok(&world, producer().spec()); // a x1 + b remain
    detach_a2.dispose().await;
    start_ok(&world, producer().spec()); // b remains
    mount.fiber().dispose().await; // detaches b with its fiber (HMR safety)
    assert!(
        start_err(&world, producer().spec())
            .to_string()
            .contains("no job controller serves this agent")
    );
}

// -------------------------------------------------------------- LocalJobRegistry events

#[tokio::test]
async fn announces_registration_progress_stopping_and_settlement_with_the_committed_projection() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "alice", None).await;
    let seen = collect(
        &world,
        owner_filter(&owner.id()),
        Some(&["registered", "progress", "stopping", "settled"]),
    );

    let mut p = owned_by(&owner);
    let id = start_ok(&world, p.spec());
    // Registration is announced only once the record is readable.
    {
        let events = lock(&seen);
        assert_eq!(events.len(), 1);
        let job = event_job(&events[0]);
        assert!(matches!(events[0], JobEvent::Registered { .. }));
        assert_eq!((&job.id, job.status), (&id, JobStatus::Running));
        assert_eq!(
            job.owner.as_ref().map(ToString::to_string).as_deref(),
            Some("alice")
        );
    }
    assert_eq!(world.jobs().list(Some(&owner.id())).len(), 1);

    p.job().update_progress("3/10");
    {
        let events = lock(&seen);
        assert!(matches!(events[1], JobEvent::Progress { .. }));
        assert_eq!(event_job(&events[1]).progress.as_deref(), Some("3/10"));
    }

    assert_eq!(
        world.jobs().kill(&id, Some(&owner.id()), None).unwrap(),
        KillOutcome::Requested
    );
    {
        let events = lock(&seen);
        assert!(matches!(events[2], JobEvent::Stopping { .. }));
        let job = event_job(&events[2]);
        assert_eq!(
            (job.status, job.progress.as_deref()),
            (JobStatus::Stopping, Some("3/10"))
        );
    }

    p.settle(killed());
    tick().await;
    {
        let events = lock(&seen);
        assert_eq!(settled_cause(&events[3]), JobSettleCause::Kill);
        let job = event_job(&events[3]);
        assert_eq!((&job.id, job.status), (&id, JobStatus::Killed));
        assert_eq!(job.progress, None);
    }
    owner.dispose_scope().await;
}

#[tokio::test]
async fn announces_an_unowned_job_to_every_session_filter_since_every_caller_can_see_it() {
    let world = harness(json!({})).await;
    let seen = collect(
        &world,
        owner_filter(&SessionId::from("anyone")),
        Some(&["registered"]),
    );
    let id = start_ok(&world, producer().spec());
    let events = lock(&seen).clone();
    assert_eq!(events.len(), 1);
    assert_eq!(event_job(&events[0]).id, id);
    assert_eq!(event_job(&events[0]).owner, None);
}

#[tokio::test]
async fn announces_the_owner_disposal_removal_and_stays_silent_when_that_owner_had_none() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "alice", None).await;
    let bystander = live_agent(&world.ctx, "bob", None).await;
    let mut p = owned_by(&owner);
    start_ok(&world, p.spec());

    let seen = collect(&world, all_owners(), Some(&["settled", "removed"]));
    p.settle(completed());
    tick().await;
    assert_eq!(types(&seen), vec!["settled"]);

    // Disposing an owner with no records changes no visible set.
    bystander.dispose_scope().await;
    assert_eq!(types(&seen), vec!["settled"]);

    owner.dispose_scope().await;
    assert_eq!(types(&seen), vec!["settled", "removed"]);
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
}

#[tokio::test]
async fn contains_a_failing_listener_so_the_lifecycle_commit_still_stands() {
    let world = harness(json!({})).await;
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(|_| Err("observer boom".into())),
        )
        .unwrap();
    let seen = collect(&world, all_owners(), Some(&["registered"]));

    let id = start_ok(&world, producer().spec());
    assert!(is_job_id("bash", &id));
    assert_eq!(lock(&seen).len(), 1);
    assert!(
        world
            .warnings()
            .iter()
            .any(|w| w.contains("event listener threw on registered"))
    );
}

#[tokio::test]
async fn announces_the_stopping_transition_during_owner_teardown_before_settlement() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "alice", None).await;
    let mut p = owned_by(&owner);
    start_ok(&world, p.spec());

    let statuses = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = statuses.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            owner_filter(&owner.id()),
            Arc::new(move |event| {
                if !matches!(event, JobEvent::Output { .. }) {
                    lock(&sink).push(format!(
                        "{}:{}",
                        event_type(event),
                        serde_json::to_value(event_job(event).status)
                            .unwrap()
                            .as_str()
                            .unwrap()
                    ));
                }
                Ok(())
            }),
        )
        .unwrap();

    // A slow producer keeps teardown parked between cancel and settlement; an observer must not
    // be left showing `running` for that whole window.
    let scope = owner.clone();
    let disposal = tokio::spawn(async move { scope.dispose_scope().await });
    tick().await;
    assert_eq!(*lock(&statuses), vec!["stopping:stopping"]);

    p.settle(killed());
    disposal.await.unwrap();
    // Settlement, then the removal that empties the visible set.
    assert_eq!(
        *lock(&statuses),
        vec!["stopping:stopping", "settled:killed", "removed:killed"]
    );
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
}

#[tokio::test]
async fn delivers_scoped_subscriptions_to_the_owning_composition_only() {
    let world = harness(json!({})).await;
    let mount = scope_in(&world);
    let other_mount = scope_in(&world);
    let alice = live_agent(&world.ctx, "alice", scope_of(&mount.ctx)).await;
    let bob = live_agent(&world.ctx, "bob", scope_of(&other_mount.ctx)).await;
    let everyone = collect(&world, all_owners(), Some(&["output"]));
    let scoped = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = scoped.clone();
    let scoped_ctx = mount.ctx.child("plugin").unwrap();
    world
        .jobs()
        .events()
        .subscribe(
            &scoped_ctx,
            scope_owners(),
            Arc::new(move |event| {
                if let JobEvent::Output { id, .. } = event {
                    lock(&sink).push(id.to_string());
                }
                Ok(())
            }),
        )
        .unwrap();
    let mut pa = owned_by(&alice);
    let alice_id = start_ok(&world, pa.spec());
    let mut pb = owned_by(&bob);
    start_ok(&world, pb.spec());
    pa.job().append("for alice observers");
    pb.job().append("for bob observers");
    assert_eq!(lock(&everyone).len(), 2);
    assert_eq!(*lock(&scoped), vec![alice_id.to_string()]);
}

// ----------------------------------------------------------- LocalJobRegistry output ring

fn texts(read: &hoh_jobs::JobOutputRead) -> Vec<String> {
    read.chunks.iter().map(|chunk| chunk.text.clone()).collect()
}

#[tokio::test]
async fn appends_advance_absolute_offsets_and_reach_observers_as_output_events() {
    let world = harness(json!({})).await;
    let outputs = collect(&world, all_owners(), Some(&["output"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    p.job().append("hello ");
    p.job().append_with_options(
        "world",
        hoh_jobs::JobAppendOptions {
            channel: Some(JobChannel::Stderr),
            gap_before: false,
        },
    );
    let first = jobs.read_at(&id, 0.0, None).unwrap();
    assert_eq!(
        first,
        hoh_jobs::JobOutputRead {
            chunks: vec![
                chunk(0, "hello ", None),
                chunk(6, "world", Some(JobChannel::Stderr))
            ],
            next: 11,
            lossy: false,
        }
    );
    // A second identical read proves nothing was consumed.
    assert_eq!(jobs.read_at(&id, 0.0, None).unwrap(), first);
    // Resuming from `next` yields nothing until more output arrives.
    assert!(
        jobs.read_at(&id, first.next as f64, None)
            .unwrap()
            .chunks
            .is_empty()
    );
    p.job().append("!");
    assert_eq!(
        jobs.read_at(&id, first.next as f64, None).unwrap().chunks,
        vec![chunk(11, "!", None)]
    );
    let output = jobs.get(&id, None).unwrap().output;
    assert_eq!(
        (output.total, output.earliest, output.spill_paths),
        (12, 0, None)
    );
    let output_event = |total| JobEvent::Output {
        id: id.clone(),
        owner: None,
        total,
    };
    assert_eq!(
        *lock(&outputs),
        vec![output_event(6), output_event(11), output_event(12)]
    );
}

#[tokio::test]
async fn staged_starter_writes_surface_at_the_registration_commit_without_observer_signals() {
    let world = harness(json!({})).await;
    let seen = collect(&world, all_owners(), None);
    let (settle, done) =
        tokio::sync::oneshot::channel::<Result<hoh_jobs::JobOutcome, hoh_error::BoxError>>();
    let id = start_ok(
        &world,
        hoh_jobs::JobSpec {
            kind: "bash".to_owned(),
            label: "staged writes".to_owned(),
            owner: None,
            tool_call: None,
            output_limit_bytes: None,
            output: None,
            run: Box::new(move |job| {
                job.append_with_options(
                    "early ",
                    hoh_jobs::JobAppendOptions {
                        channel: Some(JobChannel::Stdout),
                        gap_before: false,
                    },
                );
                job.update_progress("booting");
                Ok(hoh_jobs::JobHooks {
                    cancel: Box::new(|_| Ok(())),
                    done: Box::pin(async move { done.await.unwrap_or_else(|_| Ok(completed())) }),
                })
            }),
        },
    );
    // Only the registration commit announced anything.
    assert_eq!(types(&seen), vec!["registered"]);
    let view = world.jobs().get(&id, None).unwrap();
    assert_eq!(view.progress.as_deref(), Some("booting"));
    assert_eq!(view.output.total, 6);
    assert_eq!(
        world.jobs().read_at(&id, 0.0, None).unwrap().chunks,
        vec![chunk(0, "early ", Some(JobChannel::Stdout))]
    );
    settle.send(Ok(completed())).unwrap();
    tick().await;
}

#[tokio::test]
async fn drops_an_empty_append_without_waking_observers() {
    let world = harness(json!({})).await;
    let outputs = collect(&world, all_owners(), Some(&["output"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.job().append("");
    assert!(lock(&outputs).is_empty());
    assert_eq!(world.jobs().get(&id, None).unwrap().output.total, 0);
}

#[tokio::test]
async fn rejects_a_negative_or_fractional_read_offset() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    for offset in [-1.0, 0.5] {
        let error = jobs.read_at(&id, offset, None).unwrap_err();
        assert!(
            error.to_string().contains("invalid output read offset"),
            "{error}"
        );
    }
    assert!(
        jobs.read_at(&job_id("bash-99"), 0.0, None)
            .unwrap_err()
            .to_string()
            .contains("unknown job")
    );
}

#[tokio::test]
async fn keeps_the_spill_file_each_source_currently_reports_as_job_metadata_beyond_any_chunk() {
    use hoh_jobs::{JobOutputSource, JobSourceRead};
    let world = harness(json!({ "retainBytes": 4, "pumpPollMs": 5 })).await;
    let withdrawn = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = withdrawn.clone();
    let out = JobOutputSource {
        channel: Some(JobChannel::Stdout),
        read: Box::new(move |from| {
            Ok(if from == 0 {
                JobSourceRead {
                    text: "abcdefgh".to_owned(),
                    next_offset: 8,
                    lossy: false,
                    spill_path: Some("/spill/out.log".to_owned()),
                }
            } else {
                JobSourceRead {
                    text: String::new(),
                    next_offset: from,
                    lossy: false,
                    spill_path: (!flag.load(Ordering::SeqCst)).then(|| "/spill/out.log".to_owned()),
                }
            })
        }),
    };
    let err = JobOutputSource {
        channel: Some(JobChannel::Stderr),
        read: Box::new(|from| {
            Ok(JobSourceRead {
                text: String::new(),
                next_offset: from,
                lossy: false,
                spill_path: Some("/spill/err.log".to_owned()),
            })
        }),
    };
    let mut p = producer_with(ProducerOptions {
        output: Some(vec![out, err]),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    // The first drain ran inside start(): the live cap already evicted the head, yet the
    // projection still names the files — no chunk carried them.
    let output = world.jobs().get(&id, None).unwrap().output;
    assert_eq!((output.total, output.earliest), (8, 4));
    assert_eq!(
        output.spill_paths,
        Some(vec![
            "/spill/out.log".to_owned(),
            "/spill/err.log".to_owned()
        ])
    );
    assert_eq!(
        world.jobs().read_at(&id, 0.0, None).unwrap().chunks,
        vec![JobChunk {
            at: 4,
            text: "efgh".to_owned(),
            channel: Some(JobChannel::Stdout),
            gap_before: true,
        }]
    );

    // A source that stops naming its file (a failed seal) withdraws it on the next drain.
    withdrawn.store(true, Ordering::SeqCst);
    real_sleep(40).await;
    assert_eq!(
        world.jobs().get(&id, None).unwrap().output.spill_paths,
        Some(vec!["/spill/err.log".to_owned()])
    );
    p.settle(completed());
    tick().await;
}

#[tokio::test]
async fn announces_registration_before_the_pump_drains_a_source_that_is_readable_at_once() {
    use hoh_jobs::{JobOutputSource, JobSourceRead};
    let world = harness(json!({ "pumpPollMs": 5 })).await;
    let events = collect(&world, all_owners(), None);
    let source = JobOutputSource {
        channel: None,
        read: Box::new(|from| {
            Ok(if from == 0 {
                JobSourceRead {
                    text: "boot".to_owned(),
                    next_offset: 4,
                    lossy: false,
                    spill_path: None,
                }
            } else {
                JobSourceRead {
                    text: String::new(),
                    next_offset: from,
                    lossy: false,
                    spill_path: None,
                }
            })
        }),
    };
    let mut p = producer_with(ProducerOptions {
        output: Some(vec![source]),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    // The pump's first drain runs synchronously inside start(); the job's first event is still
    // its registration.
    assert_eq!(types(&events)[..2], ["registered", "output"]);
    assert_eq!(
        texts(&world.jobs().read_at(&id, 0.0, None).unwrap()),
        vec!["boot"]
    );
    p.settle(completed());
    tick().await;
}

#[tokio::test]
async fn evicts_whole_head_chunks_past_the_live_cap_and_flags_stale_readers_lossy() {
    let world = harness(json!({ "retainBytes": 8 })).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    p.job().append("aaaa");
    p.job().append("bbbb");
    p.job().append("cccc");
    let read = jobs.read_at(&id, 0.0, None).unwrap();
    assert!(read.lossy);
    assert_eq!(
        read.chunks,
        vec![chunk(4, "bbbb", None), chunk(8, "cccc", None)]
    );
    assert_eq!(read.next, 12);
    let output = jobs.get(&id, None).unwrap().output;
    assert_eq!((output.total, output.earliest), (12, 4));
    // A reader at the retained boundary is not lossy.
    assert!(!jobs.read_at(&id, 4.0, None).unwrap().lossy);
    // The model cursor is subject to the same eviction.
    assert!(jobs.read(&id, None).unwrap().lossy);
}

#[tokio::test]
async fn settlement_keeps_the_unconsumed_stream_for_the_model_the_terminal_read_trims_to_the_settled_cap_ends_the_stream_and_later_writes_drop()
 {
    let world = harness(json!({ "retainBytes": 1024, "settledRetainBytes": 4 })).await;
    let outputs = collect(&world, all_owners(), Some(&["output"]));
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    p.job().append("abcdefgh");
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    tick().await;
    let totals: Vec<i64> = lock(&outputs)
        .iter()
        .map(|event| match event {
            JobEvent::Output { total, .. } => *total as i64,
            _ => -1,
        })
        .collect();
    assert_eq!(totals, vec![8, 8]);
    // Nothing the model has not read is trimmed at settlement: a job that finishes before its
    // first read still hands over everything the live cap retained, and observers see the same
    // bytes.
    let view = jobs.get(&id, None).unwrap();
    assert_eq!(view.status, JobStatus::Completed);
    assert_eq!(view.detail.as_deref(), Some("exit code: 0"));
    assert_eq!((view.output.total, view.output.earliest), (8, 0));
    let at = jobs.read_at(&id, 0.0, None).unwrap();
    assert!(!at.lossy);
    assert_eq!(at.chunks, vec![chunk(0, "abcdefgh", None)]);
    let read = jobs.read(&id, None).unwrap();
    assert!(!read.lossy);
    assert_eq!(read.chunks, vec![chunk(0, "abcdefgh", None)]);
    // The terminal read consumed the stream: retention drops to the settled cap.
    let output = jobs.get(&id, None).unwrap().output;
    assert_eq!((output.total, output.earliest), (8, 4));
    let read = jobs.read_at(&id, 0.0, None).unwrap();
    assert!(read.lossy);
    assert_eq!(
        read.chunks,
        vec![JobChunk {
            at: 4,
            text: "efgh".to_owned(),
            channel: None,
            gap_before: true,
        }]
    );
    // The stream ended with the settlement: a trailing flush cannot break teardown.
    p.job().append("late");
    p.job().update_progress("late progress");
    let warnings = world.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("append to settled job"));
    assert!(warnings[1].contains("progress update on settled job"));
    assert_eq!(lock(&outputs).len(), 2);
    assert_eq!(jobs.get(&id, None).unwrap().progress, None);
}

#[tokio::test]
async fn a_settlement_after_the_model_consumed_the_stream_trims_to_the_settled_cap_at_once() {
    let world = harness(json!({ "retainBytes": 1024, "settledRetainBytes": 4 })).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    p.job().append("abcdefgh");
    assert_eq!(
        jobs.read(&id, None).unwrap().chunks,
        vec![chunk(0, "abcdefgh", None)]
    );
    p.settle(completed());
    tick().await;
    let output = jobs.get(&id, None).unwrap().output;
    assert_eq!((output.total, output.earliest), (8, 4));
    // The cursor sits at the end: nothing new, and nothing lost behind it.
    let read = jobs.read(&id, None).unwrap();
    assert!(read.chunks.is_empty());
    assert!(!read.lossy);
}

#[tokio::test]
async fn settlement_clears_the_progress_line_the_terminal_detail_is_the_outcome_alone() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    p.job().update_progress("3/10 agents done");
    assert_eq!(
        world.jobs().get(&id, None).unwrap().progress.as_deref(),
        Some("3/10 agents done")
    );
    p.settle(completed());
    tick().await;
    let view = world.jobs().get(&id, None).unwrap();
    assert_eq!(view.status, JobStatus::Completed);
    assert_eq!(view.progress, None);
    assert_eq!(view.detail, None);
}

#[tokio::test]
async fn a_teardown_forced_settlement_also_ends_the_ring() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let mut p = owned_by(&owner);
    let id = start_ok(&world, p.spec());
    p.job().append("partial");
    let scope = owner.clone();
    let disposal = tokio::spawn(async move { scope.dispose_scope().await });
    p.settle(killed());
    disposal.await.unwrap();
    assert!(
        world
            .jobs()
            .read_at(&id, 0.0, Some(&owner.id()))
            .unwrap_err()
            .to_string()
            .contains("unknown job")
    );
    // The producer face outlives the row; its trailing flush is dropped, not thrown.
    p.job().append("late");
    assert!(
        world
            .warnings()
            .iter()
            .any(|w| w.contains("append to settled job"))
    );
}

#[tokio::test]
async fn hands_out_fresh_projections_never_live_registry_state() {
    let world = harness(json!({})).await;
    let mut p = producer();
    let id = start_ok(&world, p.spec());
    let before = world.jobs().get(&id, None).unwrap();
    p.job().append("grow");
    assert_eq!(before.output.total, 0);
    assert_eq!(world.jobs().get(&id, None).unwrap().output.total, 4);
}

// -------------------------------------------------------- LocalJobRegistry pull sources

#[tokio::test]
async fn pumps_the_spec_sources_into_the_ring_and_drains_them_once_more_before_settlement() {
    use hoh_jobs::{JobOutputSource, JobSourceRead};
    let world = harness(json!({ "pumpPollMs": 1 })).await;
    let out = scripted_source(vec!["a", "bc"], Some(JobChannel::Stdout));
    // stderr only becomes readable once the producer has settled: the pump's final drain is the
    // only read that can collect it.
    let finishing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = finishing.clone();
    let err_offset = std::sync::Mutex::new(0u64);
    let err = JobOutputSource {
        channel: Some(JobChannel::Stderr),
        read: Box::new(move |_| {
            let mut offset = lock(&err_offset);
            let text = if flag.load(Ordering::SeqCst) && *offset == 0 {
                "E"
            } else {
                ""
            };
            *offset += text.len() as u64;
            Ok(JobSourceRead {
                text: text.to_owned(),
                next_offset: *offset,
                lossy: false,
                spill_path: None,
            })
        }),
    };
    let mut p = producer_with(ProducerOptions {
        output: Some(vec![out, err]),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    real_sleep(40).await;
    assert_eq!(
        texts(&world.jobs().read_at(&id, 0.0, None).unwrap()),
        vec!["a", "bc"]
    );

    let settled = collect(&world, all_owners(), Some(&["settled"]));
    finishing.store(true, Ordering::SeqCst);
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    real_sleep(40).await;
    // The final drain landed before the settlement announced itself.
    assert_eq!(lock(&settled).len(), 1);
    assert_eq!(
        texts(&world.jobs().read_at(&id, 0.0, None).unwrap()),
        vec!["a", "bc", "E"]
    );
    let view = world.jobs().get(&id, None).unwrap();
    assert_eq!((view.status, view.output.total), (JobStatus::Completed, 4));
}

#[tokio::test]
async fn contains_a_failing_source_warns_once_keeps_the_job_live_and_settles_on_the_producer() {
    use hoh_jobs::JobOutputSource;
    let world = harness(json!({ "pumpPollMs": 1 })).await;
    let broken = JobOutputSource {
        channel: Some(JobChannel::Stdout),
        read: Box::new(|_| Err("reader boom".into())),
    };
    let healthy = scripted_source(vec!["ok"], Some(JobChannel::Stderr));
    let mut p = producer_with(ProducerOptions {
        output: Some(vec![broken, healthy]),
        ..ProducerOptions::default()
    });
    let id = start_ok(&world, p.spec());
    real_sleep(40).await;
    assert_eq!(
        world.jobs().get(&id, None).unwrap().status,
        JobStatus::Running
    );
    assert_eq!(
        world.jobs().read_at(&id, 0.0, None).unwrap().chunks,
        vec![chunk(0, "ok", Some(JobChannel::Stderr))]
    );
    let warnings = world.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains(&format!("output source for {id} failed")));
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    real_sleep(40).await;
    let view = world.jobs().get(&id, None).unwrap();
    assert_eq!(view.status, JobStatus::Completed);
    assert_eq!(view.detail.as_deref(), Some("exit code: 0"));
}

#[tokio::test]
async fn stops_pumping_silently_after_a_forced_settlement() {
    use hoh_jobs::{JobOutputSource, JobSourceRead};
    let world = harness(json!({ "pumpPollMs": 1 })).await;
    let owner = live_agent(&world.ctx, "owner", None).await;
    let source = JobOutputSource {
        channel: None,
        read: Box::new(|from| {
            Ok(JobSourceRead {
                text: "x".to_owned(),
                next_offset: from + 1,
                lossy: false,
                spill_path: None,
            })
        }),
    };
    let mut p = producer_with(ProducerOptions {
        label: Some("endless"),
        owner: Some(owner.id()),
        output: Some(vec![source]),
        cancel: Some(Box::new(|_| Err("cancel boom".into()))),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());
    real_sleep(20).await;
    owner.dispose_scope().await;
    real_sleep(20).await;
    // The forced failure is the only warning: the pump's later drains are silent.
    let warnings = world.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("work may be orphaned"));
}

// ------------------------------------------------ owned job Session lifecycle records

fn killing() -> ProducerOptions {
    ProducerOptions {
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    }
}

#[tokio::test]
async fn records_lifecycle_commits_before_synchronous_hub_reactions() {
    use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "ordered-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let session = owner.session();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            all_owners(),
            Arc::new(move |event| {
                let kind = match event {
                    JobEvent::Registered { .. } => "registered",
                    JobEvent::Settled { .. } => "settled",
                    _ => return Ok(()),
                };
                let message = create_user_message(NewUserMessage {
                    content: vec![ContentBlock::text(kind)],
                    source: MessageSource::new("user"),
                });
                session.append(
                    "agent/inbox/spliced",
                    json!({
                        "target": "next-step",
                        "start": 0,
                        "inserted": [serde_json::to_value(&message)?],
                    }),
                    None,
                )?;
                Ok(())
            }),
        )
        .unwrap();
    let id = start_ok(&world, p.spec());
    p.settle(completed());
    world
        .jobs()
        .wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    assert_eq!(
        session_event_types(&owner.session()),
        vec![
            "job/registered",
            "agent/inbox/spliced",
            "job/settled",
            "agent/inbox/spliced"
        ]
    );
}

/// The source pins the original timestamps with a `Date.now` mock; Rust has no clock to mock, so
/// the records are compared with the registry's own projections instead.
#[tokio::test]
async fn publishes_original_timestamps_and_nested_tool_identity_without_consuming_output() {
    use hoh_jobs::JobToolCall;
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "journal-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let mut spec = p.spec();
    spec.tool_call = Some(JobToolCall {
        call_id: hoh_llm::ToolCallId::from("root:ptc:1"),
        root_call_id: hoh_llm::ToolCallId::from("root"),
    });
    let id = start_ok(&world, spec);
    p.job().append("unread output");
    tick().await;
    let started_at = world.jobs().get(&id, Some(&owner.id())).unwrap().started_at;
    let registered = session_event(&owner.session(), "job/registered").expect("registered record");
    assert_eq!(
        registered.data(),
        &json!({
            "jobId": id.to_string(),
            "owner": owner.id().to_string(),
            "kind": "bash",
            "label": "sleep 60",
            "startedAt": started_at,
            "toolCall": { "callId": "root:ptc:1", "rootCallId": "root" },
        })
    );
    p.settle(hoh_jobs::JobOutcome {
        detail: Some("exit code: 0".to_owned()),
        ..completed()
    });
    world
        .jobs()
        .wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    let finished_at = world
        .jobs()
        .get(&id, Some(&owner.id()))
        .unwrap()
        .finished_at
        .unwrap();
    let settled = session_event(&owner.session(), "job/settled").expect("settled record");
    let mut expected = registered.data().clone();
    for (key, value) in [
        ("finishedAt", json!(finished_at)),
        ("status", json!("completed")),
        ("cause", json!("producer")),
        ("producerReported", json!(true)),
        ("detail", json!("exit code: 0")),
    ] {
        expected[key] = value;
    }
    assert_eq!(settled.data(), &expected);
    assert_eq!(
        texts(&hoh_jobs::JobOutputRead {
            chunks: world.jobs().read(&id, Some(&owner.id())).unwrap().chunks,
            next: 0,
            lossy: false,
        }),
        vec!["unread output"]
    );
    assert!(owner.session().derive_messages().unwrap().is_empty());
}

#[tokio::test]
async fn records_cancellation_only_after_producer_settlement() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "cancel-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let id = start_ok(&world, p.spec());
    let jobs = world.jobs();
    assert_eq!(
        jobs.kill(&id, Some(&owner.id()), Some("requested by user"))
            .unwrap(),
        KillOutcome::Requested
    );
    // The cancel hook settled the producer already; the settlement itself is asynchronous.
    assert!(session_event(&owner.session(), "job/settled").is_none());
    jobs.wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    let settled = session_event(&owner.session(), "job/settled").expect("settled record");
    let data = settled.data();
    assert_eq!(data["status"], "killed");
    assert_eq!(data["cause"], "kill");
    assert_eq!(data["producerReported"], true);
    assert_eq!(data["detail"], "requested by user");
}

#[tokio::test]
async fn records_a_producer_reported_failed_outcome() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "failed-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let id = start_ok(&world, p.spec());
    p.settle(failed("operation failed after cleanup"));
    world
        .jobs()
        .wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    let settled = session_event(&owner.session(), "job/settled").expect("settled record");
    let data = settled.data();
    assert_eq!(
        (&data["status"], &data["cause"], &data["producerReported"]),
        (&json!("failed"), &json!("producer"), &json!(true))
    );
}

#[tokio::test]
async fn records_absence_of_a_producer_report_when_its_done_future_fails() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "rejected-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let id = start_ok(&world, p.spec());
    p.reject("producer rejected");
    world
        .jobs()
        .wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    let settled = session_event(&owner.session(), "job/settled").expect("settled record");
    let data = settled.data();
    assert_eq!(
        (&data["status"], &data["cause"], &data["producerReported"]),
        (&json!("failed"), &json!("producer"), &json!(false))
    );
}

#[tokio::test]
async fn preserves_a_forced_teardown_record_against_late_completion() {
    let world = harness(json!({})).await;
    let owner = live_agent(&world.ctx, "teardown-owner", None).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        cancel: Some(Box::new(|_| Err("cancel failed".into()))),
        ..ProducerOptions::default()
    });
    start_ok(&world, p.spec());
    owner.dispose_scope().await;
    p.settle(completed());
    tick().await;
    let events: Vec<_> = owner
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "job/settled")
        .cloned()
        .collect();
    assert_eq!(events.len(), 1);
    let data = events[0].data();
    assert_eq!(
        (&data["status"], &data["cause"], &data["producerReported"]),
        (&json!("failed"), &json!("teardown"), &json!(false))
    );
    assert!(world.jobs().list(Some(&owner.id())).is_empty());
}

#[tokio::test]
async fn keeps_job_identity_distinct_across_fresh_registries_with_the_same_owner() {
    let first_world = harness(json!({})).await;
    let second_world = harness(json!({})).await;
    let mut ids = Vec::new();
    let mut executions = Vec::new();
    for world in [&first_world, &second_world] {
        let owner = live_agent(&world.ctx, "same-owner", None).await;
        let mut p = producer_with(ProducerOptions {
            owner: Some(owner.id()),
            ..killing()
        });
        let id = start_ok(world, p.spec());
        ids.push(id.clone());
        p.settle(completed());
        world
            .jobs()
            .wait(&id, 1000.0, Some(&owner.id()), None)
            .await
            .unwrap();
        let registered = session_event(&owner.session(), "job/registered").expect("registration");
        executions.push(registered.data()["jobId"].as_str().unwrap().to_owned());
    }
    let id_texts: Vec<String> = ids.iter().map(ToString::to_string).collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(executions, id_texts);
}

#[tokio::test]
async fn reports_append_failures_without_breaking_execution_or_waiter_release() {
    use hoh_session::{PrepareSessionOptions, SessionStore};
    let world = harness(json!({})).await;
    // A session owned by a store but never attached to it refuses every append — the Rust way to
    // make `session.append` fail (the source spies on it).
    let detached = SessionStore::new()
        .prepare(
            Some(SessionId::from("record-error-owner")),
            PrepareSessionOptions::default(),
        )
        .unwrap();
    let owner = stub_agent_on(&world.ctx, "record-error-owner", None, detached);
    register_agent(&world.ctx, &owner).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..killing()
    });
    let id = start_ok(&world, p.spec());
    p.settle(completed());
    let view = world
        .jobs()
        .wait(&id, 1000.0, Some(&owner.id()), None)
        .await
        .unwrap();
    assert_eq!(view.status, JobStatus::Completed);
    let warnings = world.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("failed to record lifecycle"))
    );
}

// ------------------------------------------------ loader-composition.spec (config row)

/// The source mounts the package through a real Loader from a Cordis row; the Loader belongs to
/// another crate. What this package owns is that a config row reaches the provider and its
/// admission limit applies.
#[tokio::test]
async fn applies_the_provider_owned_admission_config_from_a_row() {
    let world = jobs_only().await;
    // Recreate the composition with the row's config.
    let row = bare();
    row.ctx
        .plugin::<LocalJobRegistryPlugin>(json!({ "maxConcurrentJobsPerOwner": 1 }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    drop(world);
    row.jobs()
        .attach_controller(&row.ctx, "loader-test")
        .unwrap();
    let mut hold = producer_with(ProducerOptions {
        label: Some("hold loader slot"),
        ..killing()
    });
    start_ok(&row, hold.spec());
    let mut blocked = producer_with(ProducerOptions {
        label: Some("blocked loader job"),
        ..ProducerOptions::default()
    });
    assert!(
        start_err(&row, blocked.spec())
            .to_string()
            .contains("(limit: 1)")
    );
}
