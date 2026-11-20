use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
};
use hoh_attachment_local::LocalAttachmentStore;
use hoh_conformance::fixtures::mcp::MultimodalSpillFixture;
use hoh_kernel::Kernel;
use hoh_tools::Tools;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_screenshot_tool_is_registered_with_the_fixed_input_schema() {
    let home = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, AgentLoopTestDependenciesOptions::default())
        .await
        .unwrap();
    ctx.plugin::<LocalAttachmentStore>(json!({"dshHome": home.path()}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<MultimodalSpillFixture>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let tools = ctx.get::<Tools>().unwrap();
    let tool = tools
        .get("mcp__fixture__inspect", None, None)
        .expect("registered");
    assert_eq!(tool.description, "Read a fixed window snapshot.");
    kernel.shutdown().await;
}
