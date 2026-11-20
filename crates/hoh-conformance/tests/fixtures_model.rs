//! The model-switch drivers act on production Agents (`hoh-agent-loop`).
use hoh_agent::{AgentOptions, Agents, CreateAgentOptions, Live};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_conformance::fixtures::model::{ModelSwitchCompactionDriver, ModelSwitchDriver};
use hoh_kernel::{Kernel, Plugin};
use hoh_session::SessionId;
use serde_json::Value;

fn options(id: &str) -> CreateAgentOptions {
    let mut options = CreateAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("deepseek-official".into())),
        model: Some(Live::Value("deepseek-v4-flash".into())),
        ..AgentOptions::default()
    });
    options
}

async fn selections<P: Plugin>(id: &str) -> usize {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, AgentLoopTestDependenciesOptions::default())
        .await
        .unwrap();
    ctx.plugin::<P>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    mount_agent_loop_test_harness(&ctx).await.unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    let created = agents.create(&ctx, options(id)).await.unwrap();
    let count = created
        .agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "model/selection")
        .count();
    kernel.shutdown().await;
    count
}

/// `agent.model.select(initial)` twice records the explicit initial choice once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_notice_driver_records_the_initial_route_once_at_creation() {
    assert_eq!(selections::<ModelSwitchDriver>("notice").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_compaction_driver_leaves_the_initial_route_alone() {
    assert_eq!(
        selections::<ModelSwitchCompactionDriver>("compaction").await,
        0
    );
}
