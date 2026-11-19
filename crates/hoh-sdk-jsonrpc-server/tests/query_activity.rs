mod support;
use hoh_agent::{AgentError, Agents, CreateAgentOptions};
use hoh_error::SharedError;
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_protocol::CommandResult;
use hoh_sdk_jsonrpc_server::{
    command_helpers::BuiltinCommandName,
    integration::{GoalActivationChanged, Goals, GoalsApi},
};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{commands::Registry, *};
struct ActiveGoal;
impl GoalsApi for ActiveGoal {
    fn is_active(&self, _: &Arc<dyn hoh_agent::Agent>) -> bool {
        true
    }
}
async fn setup(goal: bool) -> (Env, Arc<Registry>) {
    let h = Env::new().await;
    let c = Registry::mount(&h).await;
    if goal {
        h.ctx.provide::<Goals>(Arc::new(ActiveGoal)).await.unwrap();
    }
    h.initialize().await;
    c.register("plan", Some(BuiltinCommandName::Plan));
    c.register("goal", Some(BuiltinCommandName::Goal));
    (h, c)
}
fn user(text: &str) -> hoh_llm::UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}
fn success(text: &str) -> Option<CommandResult> {
    Some(CommandResult::Success {
        text: Some(text.into()),
        source_event_seq: None,
    })
}
async fn completion(h: &Env) -> Value {
    until(|| !h.peer.events("session.query.completed").is_empty()).await;
    h.peer
        .events("session.query.completed")
        .last()
        .unwrap()
        .clone()
}
#[tokio::test]
async fn returns_a_state_only_success_or_error_without_a_turn_or_idle() {
    for kind in ["success", "error"] {
        let (h, c) = setup(false).await;
        c.handler(Arc::new(move |_, _, _, _| {
            Box::pin(async move {
                Ok(Some(
                    serde_json::from_value(json!({"kind":kind,"text":"native result"})).unwrap(),
                ))
            })
        }));
        let response = h.query("activity", "/plan").await.unwrap();
        assert_eq!(response["kind"], "command");
        let completed = completion(&h).await;
        assert_eq!(completed["finalResponse"], "native result");
        assert_eq!(
            completed["finishReason"],
            if kind == "success" {
                "completed"
            } else {
                "error"
            }
        );
        assert!(lock(&h.model.requests).is_empty());
        h.close().await;
    }
}
#[tokio::test]
async fn waits_for_command_admitted_input_and_returns_its_final_model_reply() {
    let (h, c) = setup(false).await;
    lock(&h.model.texts).push_back("the plan".into());
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.steer(user("plan the task"))?;
            Ok(success("mode enabled"))
        })
    }));
    h.query("activity", "/plan task").await.unwrap();
    let completed = completion(&h).await;
    assert_eq!(completed["finalResponse"], "the plan");
    assert_eq!(completed["finishReason"], "completed");
    {
        let notices = lock(&h.peer.notices);
        assert_eq!(notices.last().unwrap().0, "session.query.completed");
        assert_eq!(notices[notices.len() - 2].1["status"], "idle");
    }
    h.close().await;
}
#[tokio::test]
async fn keeps_another_commands_model_input_out_of_a_suspended_state_only_invocation() {
    let (h, c) = setup(false).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let model_gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(model_gate.clone());
    let (e, r) = (entered.clone(), release.clone());
    c.handler(Arc::new(move |agent, raw, _, _| {
        let (e, r) = (e.clone(), r.clone());
        Box::pin(async move {
            if raw == " state" {
                e.notify_one();
                r.acquire().await.unwrap().forget();
                Ok(success("state updated"))
            } else {
                agent.steer(user("task"))?;
                Ok(success("input"))
            }
        })
    }));
    let server = h.server.clone();
    let state = tokio::spawn(async move {
        server.handle_request("session/query",json!({"sessionId":"activity","contentBlocks":[{"type":"text","text":"/plan state"}]})).await
    });
    entered.notified().await;
    h.query("activity", "/plan task").await.unwrap();
    h.model.entered.notified().await;
    assert!(h.peer.events("session.query.completed").is_empty());
    release.add_permits(1);
    state.await.unwrap().unwrap();
    assert_eq!(completion(&h).await["finalResponse"], "state updated");
    model_gate.add_permits(1);
    h.wait("activity").await;
    until(|| h.peer.events("session.query.completed").len() == 2).await;
    h.close().await;
}
async fn goal_run(text: &str) -> (Env, Arc<Registry>, Arc<tokio::sync::Semaphore>, Value) {
    let (h, c) = setup(true).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(gate.clone());
    lock(&h.model.texts).push_back(text.into());
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"create","goal":{"id":"goal-1","phase":"active"}}),
                None,
            )?;
            agent.followup(user("goal round"))?;
            Ok(success("accepted"))
        })
    }));
    let response = h.query("activity", "/goal work").await.unwrap();
    h.model.entered.notified().await;
    (h, c, gate, response)
}
fn goal_change(h: &Env, operation: &str, phase: &str) {
    h.agent("activity")
        .session()
        .append(
            "goal/change",
            if operation == "clear" {
                json!({"operation":"clear","cleared":{"id":"goal-1"}})
            } else {
                json!({"operation":operation,"goal":{"id":"goal-1","phase":phase}})
            },
            None,
        )
        .unwrap();
}
#[tokio::test]
async fn waits_for_the_last_reply_after_the_goal_becomes_complete() {
    let (h, _, gate, _) = goal_run("final reply").await;
    goal_change(&h, "complete", "complete");
    assert!(h.peer.events("session.query.completed").is_empty());
    gate.add_permits(1);
    assert_eq!(completion(&h).await["finalResponse"], "final reply");
    assert_eq!(completion(&h).await["finishReason"], "completed");
    h.close().await;
}
#[tokio::test]
async fn returns_empty_model_text_when_goal_ends_without_reply() {
    for (operation, phase, reason) in [
        ("complete", "complete", "completed"),
        ("pause", "paused", "aborted"),
        ("clear", "", "aborted"),
        ("block", "blocked", "error"),
    ] {
        let (h, c) = setup(true).await;
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *lock(&h.model.gate) = Some(gate.clone());
        lock(&h.model.texts).push_back("".into());
        c.handler(Arc::new(|agent, _, _, _| {
            Box::pin(async move {
                agent.session().append(
                    "goal/change",
                    json!({"operation":"create","goal":{"id":"goal-1","phase":"active"}}),
                    None,
                )?;
                agent.followup(user("goal round"))?;
                Ok(success("accepted"))
            })
        }));
        h.query("activity", "/goal work").await.unwrap();
        h.model.entered.notified().await;
        goal_change(&h, operation, phase);
        gate.add_permits(1);
        let end = completion(&h).await;
        assert_eq!(end["finalResponse"], "");
        assert_eq!(end["finishReason"], reason);
        h.close().await;
    }
}
#[tokio::test]
async fn preserves_goal_completion_when_clear_arrives_before_final_reply_finishes() {
    let (h, c, gate, response) = goal_run("final reply").await;
    goal_change(&h, "complete", "complete");
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"clear","cleared":{"id":"goal-1"}}),
                None,
            )?;
            Ok(success("Goal cleared."))
        })
    }));
    h.query("activity", "/goal clear").await.unwrap();
    assert_eq!(completion(&h).await["finalResponse"], "Goal cleared.");
    assert_eq!(h.peer.events("session.query.completed").len(), 1);
    gate.add_permits(1);
    h.wait("activity").await;
    until(|| h.peer.events("session.query.completed").len() == 2).await;
    let end = h
        .peer
        .events("session.query.completed")
        .last()
        .unwrap()
        .clone();
    assert_eq!(end["commandId"], response["execution"]["commandId"]);
    assert_eq!(end["finishReason"], "completed");
    assert_eq!(end["finalResponse"], "final reply");
    h.close().await;
}
#[tokio::test]
async fn lets_clear_return_while_original_goal_run_waits_for_current_reply() {
    let (h, c, gate, _) = goal_run("final reply").await;
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"clear","cleared":{"id":"goal-1"}}),
                None,
            )?;
            Ok(success("Goal cleared."))
        })
    }));
    h.query("activity", "/goal clear").await.unwrap();
    assert_eq!(completion(&h).await["finalResponse"], "Goal cleared.");
    gate.add_permits(1);
    h.wait("activity").await;
    until(|| h.peer.events("session.query.completed").len() == 2).await;
    let last = h
        .peer
        .events("session.query.completed")
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["finishReason"], "aborted");
    assert_eq!(last["finalResponse"], "final reply");
    h.close().await;
}
#[tokio::test]
async fn finishes_visibly_when_goal_persistence_failure_disarms_continuation_before_model_request()
{
    let (h, c) = setup(true).await;
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"create","goal":{"id":"goal-1","phase":"active"}}),
                None,
            )?;
            agent
                .ctx()
                .dispatch(GoalActivationChanged {
                    session_id: agent.id().clone(),
                    goal: Some(json!({"id":"goal-1","activation":"disarmed"})),
                })
                .invoke(|_| panic!("observer"));
            Ok(success("accepted"))
        })
    }));
    h.query("activity", "/goal work").await.unwrap();
    let end = completion(&h).await;
    assert_eq!(end["finishReason"], "error");
    assert!(end["finalResponse"].as_str().unwrap().contains("stopped"));
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn reports_a_goal_agent_failure_followed_by_disarmed_continuation() {
    let (h, _, gate, _) = goal_run("").await;
    let agent = h.agent("activity");
    h.ctx
        .dispatch(AgentError {
            agent: agent.clone(),
            turn: 1,
            step: 1,
            error: SharedError::new("driver failed"),
        })
        .invoke(|_| panic!("observer"));
    h.ctx
        .dispatch(GoalActivationChanged {
            session_id: agent.id().clone(),
            goal: Some(json!({"id":"goal-1","activation":"disarmed"})),
        })
        .invoke(|_| panic!("observer"));
    gate.add_permits(1);
    let end = completion(&h).await;
    assert_eq!(end["finishReason"], "error");
    assert!(end["finalResponse"].as_str().unwrap().contains("stopped"));
    h.close().await;
}
#[tokio::test]
async fn reports_host_cleared_goal_as_aborted_even_when_last_model_turn_failed() {
    let (h, _, gate, _) = goal_run("").await;
    let agent = h.agent("activity");
    h.ctx
        .dispatch(AgentError {
            agent,
            turn: 1,
            step: 1,
            error: SharedError::new("driver failed"),
        })
        .invoke(|_| panic!("observer"));
    goal_change(&h, "clear", "");
    gate.add_permits(1);
    assert_eq!(completion(&h).await["finishReason"], "aborted");
    h.close().await;
}
#[tokio::test]
async fn releases_active_query_observers_when_server_shuts_down() {
    let (h, c) = setup(false).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(gate);
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.steer(user("task"))?;
            Ok(success("submitted"))
        })
    }));
    h.query("activity", "/plan task").await.unwrap();
    h.model.entered.notified().await;
    h.server.shutdown().await.unwrap();
    assert!(h.peer.events("session.query.completed").is_empty());
    h.close().await;
}
#[tokio::test]
async fn ignores_other_sessions_and_reports_a_failing_admitted_model_operation() {
    let (h, c) = setup(false).await;
    let failure = h
        .ctx
        .on_waterfall::<hoh_agent::AgentPreStep, hoh_agent::PreStepDecision, _>(|_, _| async {
            Err("request preparation failed".into())
        })
        .unwrap();
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.steer(user("task"))?;
            Ok(success("submitted"))
        })
    }));
    let outside = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(&h.ctx, CreateAgentOptions::new("outside".into()))
        .await
        .unwrap();
    outside.dispose().await.unwrap();
    h.query("activity", "/plan task").await.unwrap();
    assert_eq!(completion(&h).await["finishReason"], "error");
    failure.dispose().await;
    h.close().await;
}
#[tokio::test]
async fn completion_preserves_source_field_order() {
    let (h, _) = setup(false).await;
    let c = h
        .server
        .query(
            serde_json::from_value(
                json!({"sessionId":"a","contentBlocks":[{"type":"text","text":"/plan"}]}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        c,
        hoh_protocol::SessionQueryResult::Command { .. }
    ));
    let end = completion(&h).await;
    assert_eq!(
        end.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["sessionId", "commandId", "finishReason", "finalResponse"]
    );
    h.close().await;
}
#[tokio::test]
async fn waits_across_goal_rounds_and_reports_configured_round_limit() {
    let (h, c) = setup(true).await;
    let first = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(first.clone());
    lock(&h.model.texts).extend(["first round".into(), "second round".into()]);
    c.handler(Arc::new(|agent,_,_,_|Box::pin(async move{agent.session().append("goal/change",json!({"operation":"create","goal":{"id":"goal-1","phase":"active","roundsStarted":0}}),None)?;agent.followup(user("first round"))?;Ok(success("accepted"))})));
    h.query("activity", "/goal work").await.unwrap();
    h.model.entered.notified().await;
    first.add_permits(1);
    h.wait("activity").await;
    assert!(h.peer.events("session.query.completed").is_empty());
    let second = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(second.clone());
    h.agent("activity").followup(user("second round")).unwrap();
    h.model.entered.notified().await;
    goal_change(&h, "block", "blocked");
    second.add_permits(1);
    let end = completion(&h).await;
    assert_eq!(end["finalResponse"], "second round");
    assert_eq!(end["finishReason"], "error");
    assert_eq!(lock(&h.model.requests).len(), 2);
    assert_eq!(h.peer.events("session.query.completed").len(), 1);
    h.close().await;
}
#[tokio::test]
async fn ends_a_goal_query_when_its_provider_unloads() {
    let (h, c) = setup(false).await;
    let provider = h.ctx.provide::<Goals>(Arc::new(ActiveGoal)).await.unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(gate.clone());
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"create","goal":{"id":"goal-1","phase":"active"}}),
                None,
            )?;
            agent.followup(user("round"))?;
            Ok(success("accepted"))
        })
    }));
    h.query("activity", "/goal work").await.unwrap();
    h.model.entered.notified().await;
    provider.dispose().await;
    assert_eq!(completion(&h).await["finishReason"], "error");
    assert_eq!(
        completion(&h).await["finalResponse"],
        "The goal service was unloaded before the goal finished."
    );
    gate.add_permits(1);
    h.close().await;
}
#[tokio::test]
async fn keeps_unrelated_plan_activity_alive_when_optional_goal_provider_unloads() {
    let (h, c) = setup(false).await;
    let provider = h.ctx.provide::<Goals>(Arc::new(ActiveGoal)).await.unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&h.model.gate) = Some(gate.clone());
    lock(&h.model.texts).push_back("plan reply".into());
    c.handler(Arc::new(|agent, _, _, _| {
        Box::pin(async move {
            agent.steer(user("task"))?;
            Ok(success("submitted"))
        })
    }));
    h.query("activity", "/plan task").await.unwrap();
    h.model.entered.notified().await;
    provider.dispose().await;
    assert!(h.peer.events("session.query.completed").is_empty());
    gate.add_permits(1);
    assert_eq!(completion(&h).await["finalResponse"], "plan reply");
    h.close().await;
}
#[tokio::test]
async fn correlates_provider_loss_before_native_command_response_arrives() {
    let (h, c) = setup(false).await;
    let provider = h.ctx.provide::<Goals>(Arc::new(ActiveGoal)).await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (e, r) = (entered.clone(), release.clone());
    c.handler(Arc::new(move |agent, _, _, _| {
        let (e, r) = (e.clone(), r.clone());
        Box::pin(async move {
            agent.session().append(
                "goal/change",
                json!({"operation":"create","goal":{"id":"goal-1","phase":"active"}}),
                None,
            )?;
            e.notify_one();
            r.acquire().await.unwrap().forget();
            Ok(success("accepted"))
        })
    }));
    let server = h.server.clone();
    let response = tokio::spawn(async move {
        server.handle_request("session/query",json!({"sessionId":"activity","contentBlocks":[{"type":"text","text":"/goal work"}]})).await
    });
    entered.notified().await;
    provider.dispose().await;
    let completed = completion(&h).await;
    release.add_permits(1);
    let response = response.await.unwrap().unwrap();
    assert_eq!(completed["commandId"], response["execution"]["commandId"]);
    assert_eq!(completed["finishReason"], "error");
    h.close().await;
}
#[tokio::test]
async fn releases_command_still_entering_handler_during_loop_disposal() {
    for shutdown in [false, true] {
        let (h, c) = setup(false).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let (e, r) = (entered.clone(), release.clone());
        c.handler(Arc::new(move |_, _, _, _| {
            let (e, r) = (e.clone(), r.clone());
            Box::pin(async move {
                e.notify_one();
                r.acquire().await.unwrap().forget();
                Ok(success("accepted"))
            })
        }));
        let server = h.server.clone();
        let query = tokio::spawn(async move {
            server.handle_request("session/query",json!({"sessionId":"activity","contentBlocks":[{"type":"text","text":"/plan"}]})).await
        });
        entered.notified().await;
        let stop = if shutdown {
            Some(h.server.shutdown())
        } else {
            None
        };
        h.loop_fiber.dispose().await;
        release.add_permits(1);
        assert!(query.await.unwrap().is_err());
        if let Some(stop) = stop {
            stop.await.unwrap();
            assert!(h.peer.events("session.query.completed").is_empty());
        } else {
            assert_eq!(completion(&h).await["finishReason"], "error");
        }
        h.close().await;
    }
}
