mod common;

use common::{code, harness};
use hoh_agent::{AgentCreated, AgentRegistryPlugin, SessionStartSource, agent_events};
use hoh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalActivationChanged, GoalBlockReason,
    GoalChangedEvent, GoalPhase, GoalServicePlugin, Goals,
};
use hoh_kernel::{Kernel, KernelError};
use hoh_session::{
    CreateSessionOptions, SessionEventEmitted, SessionForkSource, SessionId, SessionLogOffset,
    SessionPlugin, Sessions, SurfaceIntent,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn create(objective: &str) -> CreateGoalRequest {
    CreateGoalRequest {
        objective: objective.into(),
        max_goal_rounds: None,
    }
}

#[tokio::test]
async fn applies_default_and_writes_one_durable_change() {
    let h = harness(json!({})).await;
    let goal = h.goals.create(&h.agent, create("  finish  ")).unwrap();
    assert_eq!(goal.goal.objective, "finish");
    assert_eq!(goal.goal.max_goal_rounds, 256);
    assert_eq!(goal.activation, GoalActivation::Armed);
    let events = h.session.snapshot_events(None, None);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), "goal/change");
}

#[tokio::test]
async fn applies_configured_default() {
    let h = harness(json!({"defaultMaxGoalRounds":17})).await;
    assert_eq!(
        h.goals
            .create(&h.agent, create("x"))
            .unwrap()
            .goal
            .max_goal_rounds,
        17
    );
}

#[tokio::test]
async fn rejects_invalid_direct_configuration_with_goal_error() {
    for invalid in [
        json!({"defaultMaxGoalRounds":-1}),
        json!({"defaultMaxGoalRounds":1.5}),
    ] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SessionProjectionRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let error = ctx
            .plugin::<GoalServicePlugin>(invalid)
            .unwrap()
            .settled()
            .await
            .unwrap_err();
        let KernelError::Failed(error) = error else {
            panic!("expected plugin apply failure, got {error}");
        };
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref().as_ref())
                .unwrap()
                .code(),
            "GOAL_INVALID_MAX_ROUNDS"
        );
    }
}

#[tokio::test]
async fn rejects_invalid_create_input() {
    let h = harness(json!({})).await;
    let error = h.goals.create(&h.agent, create("   ")).unwrap_err();
    assert_eq!(code(&error), "GOAL_INVALID_OBJECTIVE");
    let error = h
        .goals
        .create(
            &h.agent,
            CreateGoalRequest {
                objective: "x".into(),
                max_goal_rounds: Some(0),
            },
        )
        .unwrap_err();
    assert_eq!(code(&error), "GOAL_INVALID_MAX_ROUNDS");
}

#[tokio::test]
async fn requires_exact_live_registry_instance() {
    let h = harness(json!({})).await;
    let impostor = common::agent(&h.ctx, h.session.clone());
    let error = h.goals.get(&impostor).unwrap_err();
    assert_eq!(code(&error), "GOAL_AGENT_NOT_LIVE");
}

#[tokio::test]
async fn remote_create_reuses_business_method() {
    let h = harness(json!({})).await;
    let result = h
        .goals
        .remote_export_create(&h.agent, create("remote"))
        .unwrap();
    let current = h.goals.get(&h.agent).unwrap().unwrap();
    assert_eq!(result.r#ref, current.as_ref());
}

#[tokio::test]
async fn edit_is_compare_and_set_and_rejects_empty_edits() {
    let h = harness(json!({})).await;
    let first = h.goals.create(&h.agent, create("first")).unwrap();
    let error = h
        .goals
        .edit(&h.agent, &first.as_ref(), EditGoalRequest::default())
        .unwrap_err();
    assert_eq!(code(&error), "GOAL_INVALID_EDIT");
    let second = h
        .goals
        .edit(
            &h.agent,
            &first.as_ref(),
            EditGoalRequest {
                objective: Some(" second ".into()),
                max_goal_rounds: Some(9),
            },
        )
        .unwrap();
    assert_eq!(second.goal.revision, 2);
    assert_eq!(second.goal.objective, "second");
    let error = h.goals.pause(&h.agent, &first.as_ref()).unwrap_err();
    assert_eq!(code(&error), "GOAL_STALE_REVISION");
}

#[tokio::test]
async fn supports_pause_resume_block_and_complete() {
    let h = harness(json!({})).await;
    let active = h.goals.create(&h.agent, create("lifecycle")).unwrap();
    let paused = h.goals.pause(&h.agent, &active.as_ref()).unwrap();
    assert_eq!(paused.goal.phase, GoalPhase::Paused);
    assert_eq!(paused.activation, GoalActivation::Disarmed);
    let resumed = h.goals.resume(&h.agent, &paused.as_ref()).unwrap();
    let blocked = h
        .goals
        .block(
            &h.agent,
            &resumed.as_ref(),
            GoalBlockReason {
                code: "needs-input".into(),
                message: " Ask owner ".into(),
            },
        )
        .unwrap();
    assert_eq!(
        blocked.goal.blocked_reason.as_ref().unwrap().message,
        "Ask owner"
    );
    let complete = h.goals.complete(&h.agent, &blocked.as_ref()).unwrap();
    assert_eq!(complete.goal.phase, GoalPhase::Complete);
}

#[tokio::test]
async fn completion_allows_replacement_but_resumable_goal_does_not() {
    let h = harness(json!({})).await;
    let first = h.goals.create(&h.agent, create("one")).unwrap();
    let error = h.goals.create(&h.agent, create("two")).unwrap_err();
    assert_eq!(code(&error), "GOAL_ALREADY_EXISTS");
    let complete = h.goals.complete(&h.agent, &first.as_ref()).unwrap();
    let second = h.goals.create(&h.agent, create("two")).unwrap();
    assert_ne!(complete.goal.id, second.goal.id);
}

#[tokio::test]
async fn rejects_invalid_phase_transitions() {
    let h = harness(json!({})).await;
    let active = h.goals.create(&h.agent, create("x")).unwrap();
    assert_eq!(
        code(&h.goals.resume(&h.agent, &active.as_ref()).unwrap_err()),
        "GOAL_INVALID_TRANSITION"
    );
    let paused = h.goals.pause(&h.agent, &active.as_ref()).unwrap();
    assert_eq!(
        code(&h.goals.pause(&h.agent, &paused.as_ref()).unwrap_err()),
        "GOAL_INVALID_TRANSITION"
    );
    assert_eq!(
        code(
            &h.goals
                .block(
                    &h.agent,
                    &paused.as_ref(),
                    GoalBlockReason {
                        code: "blocked".into(),
                        message: "why".into(),
                    }
                )
                .unwrap_err()
        ),
        "GOAL_INVALID_TRANSITION"
    );
}

#[tokio::test]
async fn validates_block_reason() {
    let h = harness(json!({})).await;
    let active = h.goals.create(&h.agent, create("x")).unwrap();
    for reason in [
        GoalBlockReason {
            code: "Bad_Code".into(),
            message: "why".into(),
        },
        GoalBlockReason {
            code: "valid".into(),
            message: "  ".into(),
        },
    ] {
        assert_eq!(
            code(
                &h.goals
                    .block(&h.agent, &active.as_ref(), reason)
                    .unwrap_err()
            ),
            "GOAL_INVALID_BLOCK_REASON"
        );
    }
}

#[tokio::test]
async fn enforces_round_cap_on_resume() {
    let h = harness(json!({})).await;
    let goal = h
        .goals
        .create(
            &h.agent,
            CreateGoalRequest {
                objective: "bounded".into(),
                max_goal_rounds: Some(1),
            },
        )
        .unwrap();
    h.session
        .append(
            "user/message",
            json!({
                "id":"round-1", "content":[{"type":"text","text":"go"}],
                "source":{"kind":"goal","goalId":goal.goal.id,"revision":1,"round":1}
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.goals.disarm(&h.agent).unwrap();
    let error = h.goals.resume(&h.agent, &goal.as_ref()).unwrap_err();
    assert_eq!(code(&error), "GOAL_INVALID_TRANSITION");
}

#[tokio::test]
async fn clear_uses_revisioned_tombstone_and_permits_fresh_goal() {
    let h = harness(json!({})).await;
    let first = h.goals.create(&h.agent, create("temporary")).unwrap();
    let tombstone = h.goals.clear(&h.agent, &first.as_ref()).unwrap();
    assert_eq!(tombstone.revision, 2);
    assert!(h.goals.get(&h.agent).unwrap().is_none());
    assert_ne!(
        h.goals.create(&h.agent, create("fresh")).unwrap().goal.id,
        first.goal.id
    );
}

#[tokio::test]
async fn disarm_does_not_write_a_durable_revision() {
    let h = harness(json!({})).await;
    let goal = h.goals.create(&h.agent, create("survive reload")).unwrap();
    let before = h.session.seq();
    let stopped = h.goals.disarm(&h.agent).unwrap().unwrap();
    assert_eq!(stopped.goal.revision, goal.goal.revision);
    assert_eq!(stopped.activation, GoalActivation::Disarmed);
    assert_eq!(h.session.seq(), before);
}

#[tokio::test]
async fn session_start_edge_disarms_live_activation() {
    let h = harness(json!({})).await;
    h.goals.create(&h.agent, create("restart")).unwrap();
    agent_events(&h.ctx, &h.agent)
        .serial::<AgentCreated, ()>(|agent| AgentCreated {
            agent,
            source: SessionStartSource::Resume,
            signal: None,
        })
        .await
        .unwrap();
    assert_eq!(
        h.goals.get(&h.agent).unwrap().unwrap().activation,
        GoalActivation::Disarmed
    );
}

#[tokio::test]
async fn notification_failures_are_contained_and_later_listener_runs() {
    let h = harness(json!({})).await;
    h.ctx
        .on_contained::<GoalChangedEvent>(|_| Err("observer broke".into()))
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let output = seen.clone();
    h.ctx
        .on_contained::<GoalChangedEvent>(move |event| {
            output.lock().unwrap().push(event.change.operation);
            Ok(())
        })
        .unwrap();
    h.goals.create(&h.agent, create("notify")).unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn consecutive_mutations_are_durable_and_foldable() {
    let h = harness(json!({})).await;
    let one = h.goals.create(&h.agent, create("x")).unwrap();
    let two = h
        .goals
        .edit(
            &h.agent,
            &one.as_ref(),
            EditGoalRequest {
                objective: Some("y".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let three = h.goals.pause(&h.agent, &two.as_ref()).unwrap();
    let folded = hoh_goal::fold_goal(&h.session.snapshot_events(None, None)).unwrap();
    assert_eq!(folded.goal.unwrap().revision, three.goal.revision);
}

#[tokio::test]
async fn external_goal_change_disarms_local_activation() {
    let h = harness(json!({})).await;
    let first = h.goals.create(&h.agent, create("external")).unwrap();
    h.session.append("goal/change", json!({
        "kind":"goal/change", "version":1, "operation":"edit",
        "goal":{"id":first.goal.id,"revision":2,"objective":"outside","phase":"active","maxGoalRounds":256},
        "roundsStarted":0,"createdAt":first.created_at,"updatedAt":first.updated_at
    }), None).unwrap();
    let current = h.goals.get(&h.agent).unwrap().unwrap();
    assert_eq!(current.goal.objective, "outside");
    assert_eq!(current.activation, GoalActivation::Disarmed);
}

#[tokio::test]
async fn projection_snapshot_tracks_current_and_clear() {
    let h = harness(json!({})).await;
    assert_eq!(
        h.projections.snapshot(&h.session, None).unwrap().values["goal"].as_ref(),
        &json!(null)
    );
    let goal = h.goals.create(&h.agent, create("project")).unwrap();
    assert_eq!(
        h.projections.snapshot(&h.session, None).unwrap().values["goal"]["goal"]["objective"],
        "project"
    );
    h.goals.clear(&h.agent, &goal.as_ref()).unwrap();
    assert_eq!(
        h.projections.snapshot(&h.session, None).unwrap().values["goal"].as_ref(),
        &json!(null)
    );
}

#[tokio::test]
async fn retained_projection_failure_rejects_host_access_but_not_append() {
    let h = harness(json!({})).await;
    h.goals.create(&h.agent, create("last valid")).unwrap();
    h.session
        .append(
            "goal/change",
            json!({"kind":"goal/change","version":1,"operation":"create"}),
            None,
        )
        .unwrap();
    agent_events(&h.ctx, &h.agent)
        .serial::<AgentCreated, ()>(|agent| AgentCreated {
            agent,
            source: SessionStartSource::Resume,
            signal: None,
        })
        .await
        .unwrap();
    let error = h.goals.get(&h.agent).unwrap_err();
    assert_eq!(code(&error), "GOAL_STATE_CORRUPT");
    assert_eq!(
        error.to_string(),
        "goal replay failed at session event 1: goal snapshot change must have exactly createdAt,goal,kind,operation,roundsStarted,updatedAt,version fields"
    );
    assert_eq!(
        h.projections.snapshot(&h.session, None).unwrap().values["goal"]["goal"]["objective"],
        "last valid"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_compare_and_set_mutations_leave_the_projection_healthy() {
    let h = harness(json!({})).await;
    let active = h.goals.create(&h.agent, create("race")).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let goals = h.goals.clone();
            let agent = h.agent.clone();
            let reference = active.as_ref();
            let barrier = barrier.clone();
            let results = results.clone();
            scope.spawn(move || {
                barrier.wait();
                let result = goals
                    .pause(&agent, &reference)
                    .map(|_| "ok".to_owned())
                    .unwrap_or_else(|error| code(&error).to_owned());
                results.lock().unwrap().push(result);
            });
        }
    });
    let mut results = results.lock().unwrap().clone();
    results.sort();
    assert_eq!(results, ["GOAL_STALE_REVISION", "ok"]);
    assert_eq!(
        h.goals.get(&h.agent).unwrap().unwrap().goal.phase,
        GoalPhase::Paused
    );
}

#[tokio::test]
async fn publishes_a_mutation_consistently_to_a_reentrant_session_observer() {
    let h = harness(json!({})).await;
    let observed = Arc::new(Mutex::new(None));
    let output = observed.clone();
    let goals = h.goals.clone();
    let agent = h.agent.clone();
    let owned = h.session.clone();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |event| {
            if event.session == owned && event.event.event_type() == "goal/change" {
                *output.lock().unwrap() = goals.get(&agent)?;
            }
            Ok(())
        })
        .unwrap();
    let created = h.goals.create(&h.agent, create("publish once")).unwrap();
    assert_eq!(observed.lock().unwrap().as_ref(), Some(&created));
    assert_eq!(h.goals.get(&h.agent).unwrap(), Some(created));
}

#[tokio::test]
async fn reports_the_full_activation_lifecycle_pair_sequence() {
    let h = harness(json!({})).await;
    let activations = Arc::new(Mutex::new(Vec::new()));
    let output = activations.clone();
    h.ctx
        .on_contained::<GoalActivationChanged>(move |event| {
            output.lock().unwrap().push(event.goal.clone());
            Ok(())
        })
        .unwrap();
    let first = h
        .goals
        .create(&h.agent, create("activation edges"))
        .unwrap();
    agent_events(&h.ctx, &h.agent)
        .serial::<AgentCreated, ()>(|agent| AgentCreated {
            agent,
            source: SessionStartSource::Resume,
            signal: None,
        })
        .await
        .unwrap();
    let resumed = h.goals.resume(&h.agent, &first.as_ref()).unwrap();
    let values = activations.lock().unwrap();
    assert_eq!(
        values
            .iter()
            .map(|value| value.as_ref().unwrap().activation)
            .collect::<Vec<_>>(),
        [
            GoalActivation::Armed,
            GoalActivation::Disarmed,
            GoalActivation::Armed,
        ]
    );
    assert!(
        values
            .iter()
            .all(|value| value.as_ref().unwrap().id == first.goal.id)
    );
    assert_eq!(
        values
            .iter()
            .map(|value| value.as_ref().unwrap().revision)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    assert_eq!(resumed.goal.revision, 2);
}

#[tokio::test]
async fn caller_edge_cases_preserve_the_source_contract() {
    let h = harness(json!({})).await;
    let impostor = common::agent(&h.ctx, h.session.clone());
    assert_eq!(
        code(&h.goals.create(&impostor, create("x")).unwrap_err()),
        "GOAL_AGENT_NOT_LIVE"
    );
    let first = h.goals.create(&h.agent, create("source cases")).unwrap();
    assert!(first.goal.id.as_str().starts_with("goal-"));
    assert_eq!(h.session.snapshot_events(None, None).len(), 1);
    assert_eq!(
        code(
            &h.goals
                .edit(
                    &h.agent,
                    &first.as_ref(),
                    EditGoalRequest {
                        objective: Some(" ".into()),
                        max_goal_rounds: None,
                    },
                )
                .unwrap_err()
        ),
        "GOAL_INVALID_OBJECTIVE"
    );
    let complete = h.goals.complete(&h.agent, &first.as_ref()).unwrap();
    assert_eq!(
        code(&h.goals.resume(&h.agent, &complete.as_ref()).unwrap_err()),
        "GOAL_INVALID_TRANSITION"
    );
}

#[tokio::test]
async fn objectives_use_javascript_trim_semantics_at_the_service_boundary() {
    let h = harness(json!({})).await;
    let next_line = h.goals.create(&h.agent, create("\u{85}kept")).unwrap();
    assert_eq!(next_line.goal.objective, "\u{85}kept");
    h.goals.complete(&h.agent, &next_line.as_ref()).unwrap();
    let bom = h
        .goals
        .create(&h.agent, create("\u{feff}trimmed\u{feff}"))
        .unwrap();
    assert_eq!(bom.goal.objective, "trimmed");
}

#[tokio::test]
async fn edit_preserves_a_block_reason_and_resume_removes_it() {
    let h = harness(json!({})).await;
    let active = h.goals.create(&h.agent, create("blocked edit")).unwrap();
    let blocked = h
        .goals
        .block(
            &h.agent,
            &active.as_ref(),
            GoalBlockReason {
                code: "needs-input".into(),
                message: "Ask owner".into(),
            },
        )
        .unwrap();
    let edited = h
        .goals
        .edit(
            &h.agent,
            &blocked.as_ref(),
            EditGoalRequest {
                objective: Some("blocked edit revised".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    assert_eq!(edited.goal.blocked_reason, blocked.goal.blocked_reason);
    assert!(
        h.goals
            .resume(&h.agent, &edited.as_ref())
            .unwrap()
            .goal
            .blocked_reason
            .is_none()
    );
}

#[tokio::test]
async fn edit_preserves_source_behavior_when_lowering_the_cap_below_used_rounds() {
    let h = harness(json!({})).await;
    let goal = h
        .goals
        .create(
            &h.agent,
            CreateGoalRequest {
                objective: "lower cap".into(),
                max_goal_rounds: Some(3),
            },
        )
        .unwrap();
    for round in 1..=2 {
        h.session
            .append(
                "user/message",
                json!({
                    "role":"user", "id":format!("cap-round-{round}"),
                    "content":[{"type":"text","text":"go"}],
                    "source":{"kind":"goal","goalId":goal.goal.id,"revision":1,"round":round}
                }),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
    }
    let current = h.goals.get(&h.agent).unwrap().unwrap();
    let edited = h
        .goals
        .edit(
            &h.agent,
            &current.as_ref(),
            EditGoalRequest {
                objective: None,
                max_goal_rounds: Some(1),
            },
        )
        .unwrap();
    assert_eq!(edited.rounds_started, 2);
    assert_eq!(edited.goal.max_goal_rounds, 1);
}

#[tokio::test]
async fn mutation_timestamps_do_not_move_behind_an_external_future_update() {
    let h = harness(json!({})).await;
    let first = h.goals.create(&h.agent, create("time")).unwrap();
    let future = first.updated_at + 1_000_000;
    h.session
        .append(
            "goal/change",
            json!({
                "kind":"goal/change", "version":1, "operation":"edit",
                "goal":{"id":first.goal.id,"revision":2,"objective":"future","phase":"active","maxGoalRounds":256},
                "roundsStarted":0,"createdAt":first.created_at,"updatedAt":future
            }),
            None,
        )
        .unwrap();
    let current = h.goals.get(&h.agent).unwrap().unwrap();
    let paused = h.goals.pause(&h.agent, &current.as_ref()).unwrap();
    assert_eq!(paused.updated_at, future);
    let tombstone = h.goals.clear(&h.agent, &paused.as_ref()).unwrap();
    let events = h.session.snapshot_events(None, None);
    let last = events.last().unwrap();
    assert_eq!(last.data()["clearedAt"], future);
    assert_eq!(tombstone.revision, 4);
}

#[tokio::test]
async fn restores_seeded_and_forked_goal_state_disarmed() {
    let h = harness(json!({})).await;
    let goal = h
        .goals
        .create(
            &h.agent,
            CreateGoalRequest {
                objective: "seeded".into(),
                max_goal_rounds: Some(5),
            },
        )
        .unwrap();
    for round in 1..=2 {
        h.session
            .append(
                "user/message",
                json!({
                    "id":format!("round-{round}"),
                    "role":"user",
                    "content":[{"type":"text","text":"go"}],
                    "source":{"kind":"goal","goalId":goal.goal.id,"revision":1,"round":round}
                }),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
    }
    h.goals.disarm(&h.agent).unwrap();

    let restored = h
        .sessions
        .create(
            &h.ctx,
            Some(SessionId::from("goal-restored")),
            CreateSessionOptions {
                seed: Some(h.session.snapshot_events(None, None).to_vec()),
                inherited_event_count: Some(SessionLogOffset::new(h.session.seq().get()).unwrap()),
                meta: Some(json!({"isSeeded":true})),
            },
        )
        .unwrap();
    let restored_agent = common::agent(&h.ctx, restored);
    let _restored = h
        .agents
        .register(&h.ctx, restored_agent.clone())
        .await
        .unwrap();
    let restored_goal = h.goals.get(&restored_agent).unwrap().unwrap();
    assert_eq!(restored_goal.rounds_started, 2);
    assert_eq!(restored_goal.activation, GoalActivation::Disarmed);

    let child = h
        .sessions
        .fork(
            &h.ctx,
            SessionForkSource::Session(h.session.clone()),
            None,
            Some(SessionId::from("goal-fork")),
        )
        .unwrap();
    let child_agent = common::agent(&h.ctx, child.clone());
    let _child = h
        .agents
        .register(&h.ctx, child_agent.clone())
        .await
        .unwrap();
    let child_goal = h.goals.get(&child_agent).unwrap().unwrap();
    assert_eq!(child_goal.goal.id, goal.goal.id);
    assert_eq!(child_goal.rounds_started, 2);
    assert_eq!(child_goal.activation, GoalActivation::Disarmed);
    assert_eq!(child.header()["isSeeded"], true);
    assert_eq!(child.inherited_event_count().get(), h.session.seq().get());
}

#[tokio::test]
async fn notification_warning_contains_the_broken_observer_text() {
    let h = harness(json!({})).await;
    h.ctx
        .on_contained::<GoalChangedEvent>(|_| Err("broken observer".into()))
        .unwrap();
    h.goals.create(&h.agent, create("warning")).unwrap();
    assert!(
        h.ctx
            .logger()
            .buffer()
            .iter()
            .any(|record| record.message.contains("broken observer"))
    );
}

#[tokio::test]
async fn projection_key_and_service_follow_the_goal_plugin_lifecycle() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!(null))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = ctx.get::<Sessions>().unwrap();
    let projections = ctx.get::<SessionProjections>().unwrap();
    let session = sessions
        .create(
            &ctx,
            Some(SessionId::from("goal-plugin-lifecycle")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    assert!(
        !projections
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("goal")
    );

    let fiber = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    let goals = ctx.get::<Goals>().unwrap();
    let agents = ctx.get::<hoh_agent::Agents>().unwrap();
    let agent = common::agent(&ctx, session.clone());
    let _registered = agents.register(&ctx, agent.clone()).await.unwrap();
    goals.create(&agent, create("unload")).unwrap();
    assert!(
        projections
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("goal")
    );
    fiber.dispose().await;
    assert!(ctx.get::<Goals>().is_none());
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    assert!(
        !projections
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("goal")
    );
    let fresh = sessions
        .create(
            &ctx,
            Some(SessionId::from("goal-after-unload")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    assert!(
        !projections
            .snapshot(&fresh, None)
            .unwrap()
            .values
            .contains_key("goal")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn goal_service_does_not_activate_without_the_projection_registry() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let _agents = ctx.plugin::<AgentRegistryPlugin>(json!({})).unwrap();
    let _goals = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    tokio::task::yield_now().await;
    assert!(ctx.get::<Goals>().is_none());
    kernel.shutdown().await;
}
