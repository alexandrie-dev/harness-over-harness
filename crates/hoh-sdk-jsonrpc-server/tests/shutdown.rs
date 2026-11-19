mod support;

use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AgentDisposed, AgentSessionMeta, Agents, CreateAgentOptions};
use hoh_error::{BoxError, SharedError};
use hoh_sdk_jsonrpc_server::integration::{Subagents, SubagentsApi};
use std::sync::{Arc, Mutex};
use support::{Env, lock};

struct Drain {
    descendants: Mutex<Vec<hoh_agent::AgentHandle>>,
    roots: Arc<Mutex<Vec<Vec<String>>>>,
    failure: Option<SharedError>,
}

impl SubagentsApi for Drain {
    fn root_of(&self, _: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError> {
        Err("root_of is not used by shutdown".into())
    }

    fn wait_for_tree_work(&self, _: Arc<dyn Agent>) -> BoxFuture<'static, Result<bool, BoxError>> {
        Box::pin(async { Ok(false) })
    }

    fn drain_descendants(
        &self,
        roots: Vec<Arc<dyn Agent>>,
    ) -> BoxFuture<'static, Result<(), BoxError>> {
        lock(&self.roots).push(roots.iter().map(|agent| agent.id().to_string()).collect());
        let descendants = std::mem::take(&mut *lock(&self.descendants));
        let failure = self.failure.clone();
        Box::pin(async move {
            for descendant in descendants.into_iter().rev() {
                descendant.dispose().await?;
            }
            match failure {
                Some(error) => Err(error.into()),
                None => Ok(()),
            }
        })
    }
}

#[tokio::test]
async fn releases_parked_descendants_before_their_root_at_depth_one_and_two() {
    for depth in [1, 2] {
        let h = Env::new().await;
        h.initialize().await;
        let root = h.open("main").await;
        let agents = h.ctx.get::<Agents>().unwrap();
        let mut descendants = Vec::new();
        let mut parent = root.id().clone();
        for level in 0..depth {
            let id = format!("owned-child-{level}").into();
            let handle = agents
                .create(
                    &h.ctx,
                    CreateAgentOptions {
                        session_id: Some(id),
                        meta: Some(AgentSessionMeta {
                            parent_session: Some(parent),
                            origin: Some("subagent".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            parent = handle.agent.id().clone();
            descendants.push(handle);
        }
        let outside = agents
            .create(
                &h.ctx,
                CreateAgentOptions {
                    session_id: Some("outside".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let roots = Arc::new(Mutex::new(Vec::new()));
        h.ctx
            .provide::<Subagents>(Arc::new(Drain {
                descendants: Mutex::new(descendants),
                roots: roots.clone(),
                failure: None,
            }))
            .await
            .unwrap();
        let disposed = Arc::new(Mutex::new(Vec::new()));
        let observed = disposed.clone();
        h.ctx
            .on_contained::<AgentDisposed>(move |event| {
                lock(&observed).push(event.agent.id().to_string());
                Ok(())
            })
            .unwrap();
        h.server.shutdown().await.unwrap();
        assert_eq!(*lock(&roots), vec![vec!["main".to_owned()]]);
        let order = lock(&disposed).clone();
        assert_eq!(order.last().map(String::as_str), Some("main"));
        for level in 0..depth {
            assert!(order.contains(&format!("owned-child-{level}")));
        }
        assert!(agents.get(&"main".into()).is_none());
        assert!(agents.get(outside.agent.id()).is_some());
        outside.dispose().await.unwrap();
        h.close().await;
    }
}

#[tokio::test]
async fn disposes_roots_after_descendant_drain_fails_and_preserves_the_failure() {
    let h = Env::new().await;
    h.initialize().await;
    h.open("main").await;
    let failure = SharedError::new("descendant drain failed");
    let roots = Arc::new(Mutex::new(Vec::new()));
    h.ctx
        .provide::<Subagents>(Arc::new(Drain {
            descendants: Mutex::new(Vec::new()),
            roots: roots.clone(),
            failure: Some(failure.clone()),
        }))
        .await
        .unwrap();
    let error = h.server.shutdown().await.unwrap_err();
    assert!(
        error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&failure)
    );
    assert_eq!(*lock(&roots), vec![vec!["main".to_owned()]]);
    assert!(h.ctx.get::<Agents>().unwrap().get(&"main".into()).is_none());
    h.kernel.shutdown().await;
}
