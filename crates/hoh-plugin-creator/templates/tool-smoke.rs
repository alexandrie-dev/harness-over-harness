use @CRATE@::{AuthorPlugin, TOOL};
use hoh_kernel::Kernel;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{ToolExecutionInput, ToolRuntime, Tools};
use serde_json::json;

#[tokio::test]
async fn echoes_valid_input_rejects_missing_input_and_unloads() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let plugin = ctx.plugin::<AuthorPlugin>(json!({})).unwrap();
    plugin.settled().await.unwrap();

    let abort = AbortController::new();
    let input = ToolExecutionInput::new(
        "test-call".into(),
        TOOL,
        json!({"text": "hello"}),
        abort.signal(),
    );
    let result = execute_tool_fixture(&ctx, input).await.unwrap();
    assert_eq!(result.value(), Some(&json!("hello")));

    let invalid = ToolExecutionInput::new("invalid-call".into(), TOOL, json!({}), abort.signal());
    assert!(execute_tool_fixture(&ctx, invalid).await.unwrap().is_error());

    plugin.dispose().await;
    assert!(ctx.get::<Tools>().unwrap().get(TOOL, None, None).is_none());
    kernel.shutdown().await;
}
