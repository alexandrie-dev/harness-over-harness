//! The model sees the source's `SubagentDepthError` when a provider refuses a start for depth.

mod support;
use hoh_error::BoxError;
use hoh_session::SessionId;
use hoh_subagent::{
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentError, SubagentErrorCode,
    SubagentProvider, SubagentRun,
};
use serde_json::json;
use std::sync::Arc;
use support::*;

/// Refuses every start the way the in-process providers refuse a depth overrun.
struct DepthRefusing;

impl SubagentProvider for DepthRefusing {
    fn name(&self) -> &str {
        "refusing"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            depth_limit: true,
            ..Default::default()
        }
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    fn supports_start(&self) -> bool {
        true
    }
    fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<Result<Arc<dyn SubagentRun>, BoxError>>> {
        Some(Box::pin(async {
            Err(Box::new(SubagentError::error(
                "subagent depth 1 exceeds maxDepth 0",
                SubagentErrorCode::DepthExceeded,
            )) as BoxError)
        }))
    }
}

#[tokio::test]
async fn a_depth_refusal_reaches_the_model_as_subagent_depth_error() {
    let env = Env::new().await;
    let _registration = env
        .subagents()
        .register_provider(Arc::new(DepthRefusing))
        .unwrap();
    env.mount(json!({ "provider": "refusing", "maxDepth": 0 }))
        .await;
    let parent = env.harness.create(SessionId::from("depth")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    let info = result.error().unwrap().info.as_ref().unwrap();
    assert_eq!(info.name, "SubagentDepthError");
    assert_eq!(info.code, "DEPTH_EXCEEDED");
    assert!(
        text(&result).contains("subagent depth 1 exceeds maxDepth 0"),
        "{}",
        text(&result)
    );
    env.close().await;
}
