mod support;

use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AgentError, Agents, CreateAgentOptions};
use hoh_error::{BoxError, SharedError};
use hoh_protocol::SessionWaitParams;
use hoh_sdk_jsonrpc_server::integration::{Subagents, SubagentsApi};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use support::{Env, lock};

struct WaitTree {
    depth: usize,
    calls: AtomicUsize,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
}

impl SubagentsApi for WaitTree {
    fn root_of(&self, _: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError> {
        Err("root_of is not used by wait".into())
    }

    fn wait_for_tree_work(&self, _: Arc<dyn Agent>) -> BoxFuture<'static, Result<bool, BoxError>> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let entered = self.entered.clone();
        let release = self.release.clone();
        let depth = self.depth;
        Box::pin(async move {
            if first {
                assert!(matches!(depth, 1 | 2));
                entered.notify_one();
                release.acquire().await.unwrap().forget();
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    fn drain_descendants(
        &self,
        _: Vec<Arc<dyn Agent>>,
    ) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}

async fn opened() -> Env {
    let h = Env::new().await;
    h.initialize().await;
    h.prompt("main", "first").await;
    h.wait("main").await;
    h
}

#[tokio::test]
async fn finishes_after_a_depth_one_or_two_owner_tree_reports_no_claimed_work() {
    for depth in [1, 2] {
        let h = opened().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let tree = Arc::new(WaitTree {
            depth,
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        });
        h.ctx.provide::<Subagents>(tree.clone()).await.unwrap();
        let server = h.server.clone();
        let waiting = tokio::spawn(async move {
            server
                .wait(SessionWaitParams {
                    session_id: "main".into(),
                })
                .await
        });
        entered.notified().await;
        release.add_permits(1);
        waiting.await.unwrap().unwrap();
        assert_eq!(tree.calls.load(Ordering::SeqCst), 2);
        h.close().await;
    }
}

#[tokio::test]
async fn keeps_an_unowned_descendant_failure_separate_from_the_sdk_root_outcome() {
    let h = opened().await;
    let child = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(
            &h.ctx,
            CreateAgentOptions {
                session_id: Some("child".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.ctx
        .dispatch(AgentError {
            agent: child.agent.clone(),
            turn: 1,
            step: 0,
            error: SharedError::new("delegated work failed"),
        })
        .invoke(|_| ());
    h.server
        .wait(SessionWaitParams {
            session_id: "main".into(),
        })
        .await
        .unwrap();
    child.dispose().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn rejects_a_live_failure_arriving_during_wait_and_allows_durable_recovery() {
    let h = opened().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    h.ctx
        .provide::<Subagents>(Arc::new(WaitTree {
            depth: 1,
            calls: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
        }))
        .await
        .unwrap();
    let server = h.server.clone();
    let waiting = tokio::spawn(async move {
        server
            .wait(SessionWaitParams {
                session_id: "main".into(),
            })
            .await
    });
    entered.notified().await;
    let agent = h.agent("main");
    let failure = SharedError::new("summary turn/end unavailable");
    h.ctx
        .dispatch(AgentError {
            agent: agent.clone(),
            turn: 2,
            step: 0,
            error: failure.clone(),
        })
        .invoke(|_| ());
    release.add_permits(1);
    let error = waiting.await.unwrap().unwrap_err();
    assert!(
        error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&failure)
    );
    agent
        .session()
        .append(
            "turn/end",
            json!({"turn":2,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    h.server
        .wait(SessionWaitParams {
            session_id: "main".into(),
        })
        .await
        .unwrap();
    h.close().await;
}

#[tokio::test]
async fn leaves_a_represented_turn_error_in_the_durable_outcome() {
    let h = opened().await;
    let agent = h.agent("main");
    h.ctx
        .dispatch(AgentError {
            agent: agent.clone(),
            turn: 2,
            step: 0,
            error: SharedError::new("represented failure"),
        })
        .invoke(|_| ());
    agent
        .session()
        .append(
            "turn/end",
            json!({"turn":2,"reason":{"kind":"error","error":{"message":"represented failure"}}}),
            None,
        )
        .unwrap();
    h.server
        .wait(SessionWaitParams {
            session_id: "main".into(),
        })
        .await
        .unwrap();
    h.close().await;
}

#[tokio::test]
async fn retains_an_error_emitted_during_terminal_publication() {
    let h = opened().await;
    let agent = h.agent("main");
    let failure = SharedError::new("terminal publication failed");
    let emitted = Arc::new(Mutex::new(Some(failure.clone())));
    let pending = emitted.clone();
    let ctx = h.ctx.clone();
    h.ctx
        .on_contained::<hoh_session::SessionEventEmitted>(move |event| {
            if event.event.event_type() == "turn/end"
                && let Some(error) = lock(&pending).take()
            {
                let agent = ctx
                    .get::<Agents>()
                    .unwrap()
                    .get(event.session.id())
                    .unwrap();
                ctx.dispatch(AgentError {
                    agent,
                    turn: 2,
                    step: 0,
                    error,
                })
                .invoke(|_| ());
            }
            Ok(())
        })
        .unwrap();
    agent
        .session()
        .append(
            "turn/end",
            json!({"turn":2,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    let error = h
        .server
        .wait(SessionWaitParams {
            session_id: "main".into(),
        })
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&failure)
    );
    agent
        .session()
        .append(
            "turn/end",
            json!({"turn":3,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    h.server
        .wait(SessionWaitParams {
            session_id: "main".into(),
        })
        .await
        .unwrap();
    h.close().await;
}
