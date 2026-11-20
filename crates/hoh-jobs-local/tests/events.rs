//! Port of `tests/events.spec.ts`.

mod common;

use common::{TestAgent, all_owners, bare, lock, owner_filter, scope_owners, stub_agent};
use hoh_agent::Agent;
use hoh_jobs::{JobEvent, JobId, JobOutputView, JobStatus, JobView};
use hoh_jobs_local::events::{JobEventHub, JobLayer};
use hoh_kernel::Context;
use hoh_scope::{CreateScopeOptions, ScopeKey, ScopeLayer, ScopedLayers, create_scope, scope_of};
use hoh_session::SessionId;
use std::sync::{Arc, Mutex};

fn view(owner: Option<&TestAgent>) -> JobView {
    JobView {
        id: JobId::from("bash-1"),
        kind: "bash".to_owned(),
        label: "x".to_owned(),
        owner: owner.map(TestAgent::id),
        output_limit_bytes: None,
        status: JobStatus::Running,
        progress: None,
        detail: None,
        started_at: 0,
        finished_at: None,
        output: JobOutputView {
            total: 0,
            earliest: 0,
            spill_paths: None,
        },
    }
}

fn registered(owner: Option<&TestAgent>) -> JobEvent {
    JobEvent::Registered { job: view(owner) }
}

fn hub_in(ctx: &Context) -> JobEventHub {
    let layers = ScopedLayers::new(|_| Ok(JobLayer::default()), || Ok(())).unwrap();
    JobEventHub::new(layers, ctx.clone())
}

fn owner_of(event: &JobEvent) -> Option<SessionId> {
    match event {
        JobEvent::Output { owner, .. } => owner.clone(),
        JobEvent::Registered { job } => job.owner.clone(),
        other => panic!("unexpected event {other:?}"),
    }
}

fn names(seen: &Mutex<Vec<Option<SessionId>>>) -> Vec<Option<String>> {
    lock(seen)
        .iter()
        .map(|owner| owner.as_ref().map(ToString::to_string))
        .collect()
}

fn recorder(seen: &Arc<Mutex<Vec<Option<SessionId>>>>) -> hoh_jobs::JobEventListener {
    let seen = seen.clone();
    Arc::new(move |event| {
        lock(&seen).push(owner_of(event));
        Ok(())
    })
}

fn by_agent(agent: &TestAgent) -> Option<&Arc<dyn Agent>> {
    Some(&agent.agent)
}

#[tokio::test]
async fn delivers_by_owner_that_session_plus_every_unowned_job_never_another_session() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let alice = stub_agent(&world.ctx, "alice", None);
    let bob = stub_agent(&world.ctx, "bob", None);
    let seen = Arc::new(Mutex::new(Vec::new()));
    hub.subscribe(&world.ctx, owner_filter(&alice.id()), recorder(&seen))
        .unwrap();

    hub.emit(&registered(Some(&alice)), by_agent(&alice));
    hub.emit(&registered(Some(&bob)), by_agent(&bob));
    hub.emit(&registered(None), None);
    hub.emit(
        &JobEvent::Output {
            id: JobId::from("bash-1"),
            owner: Some(bob.id()),
            total: 1,
        },
        by_agent(&bob),
    );
    hub.emit(
        &JobEvent::Output {
            id: JobId::from("bash-1"),
            owner: None,
            total: 1,
        },
        None,
    );
    assert_eq!(names(&seen), vec![Some("alice".to_owned()), None, None]);
}

#[tokio::test]
async fn delivers_all_everything_and_scope_only_the_owners_composed_under_the_subscriber() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let mount = create_scope(&world.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let other_mount =
        create_scope(&world.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let alice = stub_agent(&world.ctx, "alice", scope_of(&mount.ctx));
    let bob = stub_agent(&world.ctx, "bob", scope_of(&other_mount.ctx));

    let all = Arc::new(Mutex::new(Vec::new()));
    hub.subscribe(&world.ctx, all_owners(), recorder(&all))
        .unwrap();
    let scoped = Arc::new(Mutex::new(Vec::new()));
    let plugin_ctx = mount.ctx.child("plugin").unwrap();
    hub.subscribe(&plugin_ctx, scope_owners(), recorder(&scoped))
        .unwrap();
    let unscoped = Arc::new(Mutex::new(Vec::new()));
    hub.subscribe(&world.ctx, scope_owners(), recorder(&unscoped))
        .unwrap();

    hub.emit(&registered(Some(&alice)), by_agent(&alice));
    hub.emit(&registered(Some(&bob)), by_agent(&bob));
    hub.emit(&registered(None), None);
    let everyone = vec![Some("alice".to_owned()), Some("bob".to_owned()), None];
    assert_eq!(names(&all), everyone);
    // A preset-scoped subscription hears its own agents only; an unowned job has no chain to
    // walk, so only unscoped subscriptions hear it.
    assert_eq!(names(&scoped), vec![Some("alice".to_owned())]);
    assert_eq!(names(&unscoped), everyone);
}

#[tokio::test]
async fn contains_a_failing_listener_and_keeps_delivering_to_later_ones() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let seen = Arc::new(Mutex::new(Vec::new()));
    hub.subscribe(
        &world.ctx,
        all_owners(),
        Arc::new(|_| Err("listener boom".into())),
    )
    .unwrap();
    hub.subscribe(&world.ctx, all_owners(), recorder(&seen))
        .unwrap();
    hub.emit(&registered(None), None);
    assert_eq!(lock(&seen).len(), 1);
    let warnings = world.warnings();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("event listener threw on registered: Error: listener boom"),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn contains_a_panicking_listener_and_keeps_delivering_to_later_ones() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let seen = Arc::new(Mutex::new(Vec::new()));
    hub.subscribe(
        &world.ctx,
        all_owners(),
        Arc::new(|_| panic!("listener boom")),
    )
    .unwrap();
    hub.subscribe(&world.ctx, all_owners(), recorder(&seen))
        .unwrap();
    hub.emit(&registered(None), None);
    assert_eq!(lock(&seen).len(), 1);
    assert!(
        world.warnings()[0].contains("event listener threw on registered: Error: listener boom")
    );
}

#[tokio::test]
async fn unregisters_through_the_disposer_and_with_the_subscribing_fiber() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let tagged = |tag: u8| -> hoh_jobs::JobEventListener {
        let seen = seen.clone();
        Arc::new(move |_| {
            lock(&seen).push(tag);
            Ok(())
        })
    };
    let detach = hub.subscribe(&world.ctx, all_owners(), tagged(1)).unwrap();
    let plugin_ctx = world.ctx.child("plugin").unwrap();
    hub.subscribe(&plugin_ctx, scope_owners(), tagged(2))
        .unwrap();
    hub.subscribe(
        &plugin_ctx,
        owner_filter(&SessionId::from("nobody")),
        tagged(3),
    )
    .unwrap();

    // Filter-kind order is not a contract; the assertion only fixes it for determinism.
    hub.emit(&registered(None), None);
    assert_eq!(*lock(&seen), vec![1, 3, 2]);
    detach.dispose().await;
    hub.emit(&registered(None), None);
    assert_eq!(*lock(&seen), vec![1, 3, 2, 3, 2]);
    plugin_ctx.fiber().dispose().await;
    hub.emit(&registered(None), None);
    assert_eq!(*lock(&seen), vec![1, 3, 2, 3, 2]);
}

#[tokio::test]
async fn keeps_a_scoped_layer_alive_only_while_a_subscription_or_controller_remains_in_it() {
    let world = bare();
    let hub = hub_in(&world.ctx);
    let mount = create_scope(&world.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let plugin_ctx = mount.ctx.child("plugin").unwrap();
    hub.subscribe(&plugin_ctx, scope_owners(), recorder(&seen))
        .unwrap();
    let alice = stub_agent(&world.ctx, "alice", scope_of(&mount.ctx));
    hub.emit(&registered(Some(&alice)), by_agent(&alice));
    assert_eq!(lock(&seen).len(), 1);
    plugin_ctx.fiber().dispose().await;
    hub.emit(&registered(Some(&alice)), by_agent(&alice));
    assert_eq!(lock(&seen).len(), 1);
    assert!(JobLayer::default().is_empty());
}
