use hoh_agent_tool_presentation::{AgentToolPresentation, PresentationMode};
use hoh_kernel::{FiberState, Kernel, PathSeg, PluginDef, ValidationError, config};
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tools::ToolRuntime;
use serde_json::json;

async fn host() -> (Kernel, hoh_kernel::Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let prompt = ctx
        .plugin::<SystemPrompt>(json!({}))
        .expect("system prompt plugin should be accepted");
    prompt
        .settled()
        .await
        .expect("system prompt plugin should load");
    let tools = ctx
        .plugin::<ToolRuntime>(json!({}))
        .expect("tools plugin should be accepted");
    tools.settled().await.expect("tools plugin should load");
    (kernel, ctx)
}

fn rejected_config(value: serde_json::Value) -> ValidationError {
    let definition = PluginDef::of::<AgentToolPresentation>();
    config::validate(definition.schema(), &value)
        .expect_err("invalid selector config should be rejected")
}

#[test]
fn rejects_an_unsupported_presentation_mode_in_configuration() {
    let error = rejected_config(json!({"mode": "both"}));
    assert_eq!(error.issues.len(), 1);
    assert_eq!(error.issues[0].path, [PathSeg::Key("mode".into())]);
}

#[test]
fn declares_the_services_it_uses_without_holding_a_ptc_runtime_hostage() {
    let definition = PluginDef::of::<AgentToolPresentation>();
    assert_eq!(definition.name(), "tool-presentation");
    assert_eq!(
        definition
            .deps()
            .iter()
            .map(|dependency| dependency.name)
            .collect::<Vec<_>>(),
        ["tools"]
    );
}

#[tokio::test]
async fn keeps_the_inherited_executor_while_waiting_for_a_ptc_runtime() {
    let (kernel, ctx) = host().await;
    let inherited = ctx
        .plugin::<NativeToolExecutorPlugin>(serde_json::Value::Null)
        .expect("inherited native executor should be accepted");
    inherited
        .settled()
        .await
        .expect("inherited native executor should load");

    let selector = ctx
        .plugin::<AgentToolPresentation>(json!({"mode": "ptc"}))
        .expect("PTC selector should be accepted");
    selector
        .settled()
        .await
        .expect("selector itself should load without the PTC runtime");

    let snapshot = kernel.snapshot().await;
    let ptc = snapshot
        .fibers
        .iter()
        .find(|fiber| fiber.name.as_ref() == "ptc-tool-executor")
        .expect("selector should mount the PTC child");
    assert_eq!(ptc.state, FiberState::Pending);
    assert_eq!(
        ptc.missing.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
        ["ptcRuntime"]
    );
    assert_eq!(inherited.state(), FiberState::Active);
    kernel.shutdown().await;
}

#[test]
fn requires_a_mode_rather_than_defaulting_one() {
    let error = rejected_config(json!({}));
    assert_eq!(error.issues.len(), 1);
    assert_eq!(error.issues[0].path, [PathSeg::Key("mode".into())]);
}

#[tokio::test]
async fn native_mode_mounts_the_native_executor_child() {
    let (kernel, ctx) = host().await;
    let selector = ctx
        .plugin::<AgentToolPresentation>(json!({"mode": PresentationMode::Native}))
        .expect("native selector should be accepted");
    selector
        .settled()
        .await
        .expect("native selector should load");

    let snapshot = kernel.snapshot().await;
    assert!(snapshot.fibers.iter().any(|fiber| {
        fiber.name.as_ref() == "native-tool-executor" && fiber.state == FiberState::Active
    }));
    kernel.shutdown().await;
}
